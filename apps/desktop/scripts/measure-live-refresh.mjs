// Measures how long a file written on disk takes to show in the right sidebar's Files tree and
// Source list: a real daemon watches a scratch clone of this repository, and a production build
// of the live-refresh fixture reads it through the daemon, frame by frame from the write. Then
// a burst of writes like a build's counts the refreshes it causes.
// PLAYWRIGHT_MODULE=<installed module> CHROME_BIN=<installed browser> BRIGADIERD=<daemon binary> \
//   node scripts/measure-live-refresh.mjs [runs] [scratch dir]
import { execFileSync, spawn } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { createConnection } from 'node:net';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { build, preview } from 'vite';

const playwright = await import(process.env.PLAYWRIGHT_MODULE || 'playwright');
const { chromium } = playwright.chromium ? playwright : playwright.default;
const runs = Number(process.argv[2] ?? 10);
const scratch = process.argv[3] ?? `/tmp/live-refresh-${process.pid}`;
const root = fileURLToPath(new URL('..', import.meta.url));
const daemonBin = process.env.BRIGADIERD ?? join(root, '../../target/release/brigadierd');
const repo = join(scratch, 'repo');
const data = join(scratch, 'data');
const outDir = join(scratch, 'dist');
const COLUMN = '[data-slot="workspace-pane"]';
const FILES_KEY = { code: 'KeyE', metaKey: true, shiftKey: true };
const SOURCE_KEY = { code: 'KeyG', ctrlKey: true, shiftKey: true };
const now = () => performance.timeOrigin + performance.now();

rmSync(scratch, { recursive: true, force: true });
mkdirSync(scratch, { recursive: true });
// A checkout this repository's size, sharing its objects.
execFileSync('git', ['clone', '-q', '--shared', join(root, '../..'), repo]);

const daemon = spawn(daemonBin, ['--data-dir', data, '--foreground'], { stdio: ['ignore', 'ignore', 'pipe'] });
let daemonLog = '';
daemon.stderr.on('data', (chunk) => { daemonLog += chunk; });
const tokenPath = join(data, 'run/ipc.token');
const socketPath = join(data, 'run/brigadierd.sock');
for (let i = 0; i < 200 && !(existsSync(tokenPath) && existsSync(socketPath)); i++) await new Promise((r) => setTimeout(r, 50));

/** One connection to the daemon: length-prefixed JSON frames. */
const socket = createConnection(socketPath);
await new Promise((resolve, reject) => { socket.once('connect', resolve); socket.once('error', reject); });
const send = (frame) => {
  const body = Buffer.from(JSON.stringify(frame));
  const head = Buffer.alloc(4);
  head.writeUInt32BE(body.length);
  socket.write(Buffer.concat([head, body]));
};
const pending = new Map();
const changes = [];
let onFrame = () => {};
let buffered = Buffer.alloc(0);
socket.on('data', (chunk) => {
  buffered = Buffer.concat([buffered, chunk]);
  while (buffered.length >= 4 && buffered.length >= 4 + buffered.readUInt32BE(0)) {
    const length = buffered.readUInt32BE(0);
    const frame = JSON.parse(buffered.subarray(4, 4 + length).toString());
    buffered = buffered.subarray(4 + length);
    if (frame.type === 'response') {
      pending.get(frame.id)?.(frame.result);
      pending.delete(frame.id);
    } else {
      if (frame.type === 'workspaceChanged') changes.push(now());
      onFrame(frame);
    }
  }
});
let nextId = 0;
const ask = (request) => new Promise((resolve) => {
  const id = ++nextId;
  pending.set(id, resolve);
  send({ type: 'request', id, request });
});
send({ type: 'hello', token: readFileSync(tokenPath, 'utf8').trim(), protocol: 18, client: { name: 'live-refresh-probe', pid: process.pid } });
const added = await ask({ method: 'addProject', path: repo, name: '', init: false });
if (added.status !== 'ok') throw new Error(JSON.stringify(added));
const projectId = added.value.project.id;

// The fixture's session is this project: requests and frames are translated on the way.
const project = { type: 'project', projectId };
const reads = { getWorkspaceTree: 0, getSourceState: 0 };
const took = { getWorkspaceTree: [], getSourceState: [] };
async function forward(request) {
  if (request.method in reads) reads[request.method]++;
  const asked = now();
  let sent = { ...request };
  if (sent.target?.type === 'conversation') sent.target = project;
  if (sent.method === 'getSourceState') sent = { method: 'getProjectSourceState', projectId };
  const result = await ask(sent);
  took[request.method]?.push(now() - asked);
  if (result.status !== 'ok') return { error: result.error };
  return { value: { ...result.value, method: request.method } };
}

await build({ root, configFile: join(root, 'vite.config.ts'), logLevel: 'warn',
  build: { outDir, emptyOutDir: true, rolldownOptions: { input: { liveRefresh: join(root, 'fixtures/live-refresh.html') } } } });
const server = await preview({ root, configFile: join(root, 'vite.config.ts'), logLevel: 'warn', build: { outDir }, preview: { port: 0 } });
const browser = await chromium.launch({ headless: true, args: process.platform === 'darwin' ? ['--single-process'] : [], executablePath: process.env.CHROME_BIN });

/** In the page: the wall-clock time of the first frame whose column shows `text`. */
function shows({ column, text }) {
  return new Promise((resolve) => {
    const start = performance.now();
    const tick = () => {
      if (document.querySelector(column)?.textContent?.includes(text)) resolve(performance.timeOrigin + performance.now());
      else if (performance.now() - start > 5000) resolve(null);
      else requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  });
}

const press = (page, key) => page.evaluate((pressed) => window.dispatchEvent(new KeyboardEvent('keydown', { ...pressed, bubbles: true, cancelable: true })), key);
const pause = (ms) => new Promise((r) => setTimeout(r, ms));

/** Writes a new file at the checkout's root and times its row in the open view. */
async function once(page, name) {
  const painted = page.evaluate(shows, { column: COLUMN, text: name });
  await pause(50);
  const before = changes.length;
  const wrote = now();
  writeFileSync(join(repo, name), 'live\n');
  const at = await painted;
  const event = changes[before];
  return { paint: at === null ? NaN : Math.round(at - wrote), event: event === undefined ? NaN : Math.round(event - wrote) };
}

const results = { Files: [], Source: [] };
let storm;
try {
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
  await page.exposeFunction('daemon', forward);
  onFrame = (frame) => { page.evaluate((sent) => window.heard(sent), frame).catch(() => undefined); };
  await page.goto(`${server.resolvedUrls.local[0]}fixtures/live-refresh.html`);
  await page.locator('header[data-slot="test-toolbar"] button[aria-label="Right sidebar"]').waitFor();
  for (const [view, key, text] of [['Files', FILES_KEY, 'crates'], ['Source', SOURCE_KEY, 'Changes']]) {
    await press(page, key);
    if (await page.evaluate(shows, { column: COLUMN, text }) === null) {
      throw new Error(`${view} never showed: ${await page.evaluate((column) => document.querySelector(column)?.textContent, COLUMN)}`);
    }
    await pause(1500);
    for (let run = 0; run < runs; run++) {
      results[view].push(await once(page, `live-${view.toLowerCase()}-${run}.txt`));
      await pause(800);
    }
  }
  // A build's burst: a file under `generated/` every 20 ms for 3 s, then how many refreshes
  // it caused and whether the last file shows.
  await pause(1500);
  mkdirSync(join(repo, 'generated'));
  const [events, trees, sources] = [changes.length, reads.getWorkspaceTree, reads.getSourceState];
  const started = now();
  let last = '';
  for (let i = 0; now() - started < 3000; i++) {
    last = `burst-${i}.txt`;
    writeFileSync(join(repo, 'generated', last), `${i}\n`);
    await pause(20);
  }
  const ended = now();
  await press(page, FILES_KEY);
  const painted = await page.evaluate(shows, { column: COLUMN, text: 'generated' });
  await pause(1500);
  storm = {
    writes: Number(last.match(/\d+/)[0]) + 1,
    events: changes.length - events,
    treeReads: reads.getWorkspaceTree - trees,
    sourceReads: reads.getSourceState - sources,
    lastEventAfterEnd: Math.round(changes.at(-1) - ended),
    shown: painted !== null,
  };
} finally {
  await browser.close();
  await server.close();
  socket.destroy();
  daemon.kill('SIGTERM');
  await new Promise((r) => daemon.once('exit', r));
  rmSync(scratch, { recursive: true, force: true });
}

const sorted = (values) => values.filter(Number.isFinite).toSorted((a, b) => a - b);
const pick = (values, q) => { const s = sorted(values); return s[Math.min(s.length - 1, Math.floor(s.length * q))]; };
console.log(`${runs} writes per view, ms from the write (median / p90 / max):`);
for (const view of ['Files', 'Source']) {
  const rows = results[view];
  const paints = rows.map((r) => r.paint);
  const events = rows.map((r) => r.event);
  console.log(`${view.padEnd(6)} daemon event ${pick(events, 0.5)} / ${pick(events, 0.9)} / ${sorted(events).at(-1)}   painted ${pick(paints, 0.5)} / ${pick(paints, 0.9)} / ${sorted(paints).at(-1)}   missed ${paints.filter((p) => !Number.isFinite(p)).length}`);
}
console.log(`burst: ${storm.writes} writes in 3 s -> ${storm.events} change events, ${storm.treeReads} tree reads, ${storm.sourceReads} source reads; last event ${storm.lastEventAfterEnd} ms after the last write; final state shown ${storm.shown}`);
console.log(`daemon reads (median ms): tree ${Math.round(pick(took.getWorkspaceTree, 0.5))}, source ${Math.round(pick(took.getSourceState, 0.5))}`);
if (process.env.LIVE_REFRESH_LOG) console.log(daemonLog);
