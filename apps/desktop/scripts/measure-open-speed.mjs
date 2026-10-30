// Measures opening and switching conversations: a real release daemon on a scratch data dir with
// made-up CLI session files (per CLI a long, a short and a "running" one whose file grows, and a
// long one older than the ten the app reads ahead), and a production build of the open-speed
// fixture (the whole app) in headless Chromium, its IPC passed through as the live-refresh probe
// does. Per switch, frame by frame from `select`: when the thread is whole, frames with no thread
// or "Loading conversation", rows that move after that, the requests it made, the scroll kept.
// Phases: the long ones' not-read-ahead open per CLI, then `switches` switches per CLI; then the
// daemon is killed and started again, the page reloaded, and the same again.
// PLAYWRIGHT_MODULE=<installed module> CHROME_BIN=<installed browser> BRIGADIERD=<daemon binary> \
//   node scripts/measure-open-speed.mjs [scratch dir] [switches]
// OPEN_SPEED_JSON=<file> also writes every switch's numbers there.
import { spawn } from 'node:child_process';
import { appendFileSync, existsSync, mkdirSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { createConnection } from 'node:net';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { build, preview } from 'vite';

const playwright = await import(process.env.PLAYWRIGHT_MODULE || 'playwright');
const { chromium } = playwright.chromium ? playwright : playwright.default;
const scratch = process.argv[2] ?? `/tmp/open-speed-${process.pid}`;
const switches = Number(process.argv[3] ?? 20);
const root = fileURLToPath(new URL('..', import.meta.url));
const daemonBin = process.env.BRIGADIERD ?? join(root, '../../target/release/brigadierd');
const data = join(scratch, 'data');
const outDir = join(scratch, 'dist');
const now = () => performance.timeOrigin + performance.now();
const pause = (ms) => new Promise((r) => setTimeout(r, ms));

rmSync(scratch, { recursive: true, force: true });
mkdirSync(scratch, { recursive: true });
// The CLIs' homes, so nothing the daemon or a CLI it asks reads is the user's.
const home = join(scratch, 'home');
const homes = { claude: join(scratch, 'claude-home'), codex: join(scratch, 'codex-home'), opencode: join(scratch, 'opencode-home') };
for (const dir of [home, ...Object.values(homes)]) mkdirSync(dir, { recursive: true });

// ---- Made-up session files, in each CLI's own format.
const BASE = Date.parse('2026-10-20T09:00:00Z');
const iso = (ms) => new Date(ms).toISOString();
const words = 'the window stays put while what is inside it scrolls and only the pane under the pointer moves along'.split(' ');
const sentence = (seed, length) => Array.from({ length }, (_, i) => words[(seed * 7 + i * 3) % words.length]).join(' ');
const output = (seed, bytes) => {
  let text = '';
  for (let line = 0; text.length < bytes; line++) text += `${String(line).padStart(4)} | ${sentence(seed + line, 10)}\n`;
  return text;
};
/** One session's turns: `calls` commands per turn, each `bytes` of output; the last answer ends with `end`. */
const turnsOf = (tag, turns, calls, bytes) => Array.from({ length: turns }, (_, t) => ({
  t, at: BASE + t * 120_000, ask: `Question ${t + 1} of ${tag}: ${sentence(t, 14)}`, think: sentence(t + 1, 60),
  calls: Array.from({ length: calls }, (_, c) => ({ id: `${tag}-c${t}-${c}`, command: `pnpm test --filter part-${c} # ${tag} turn ${t + 1}`, output: output(t + c, bytes) })),
  answer: `Answer ${t + 1} of ${tag}. ${sentence(t + 2, 50)}${t === turns - 1 ? ` END-${tag}` : ''}`,
}));

const write = {
  claude(tag, turns) {
    const line = (value) => `${JSON.stringify(value)}\n`;
    return turns.map(({ t, at, ask, think, calls, answer }) => {
      let lines = line({ type: 'user', uuid: `${tag}-u${t}`, timestamp: iso(at), message: { role: 'user', content: ask } });
      lines += line({ type: 'assistant', uuid: `${tag}-r${t}`, timestamp: iso(at + 1000), message: { id: `${tag}-m${t}-think`, role: 'assistant', content: [{ type: 'thinking', thinking: think }] } });
      calls.forEach((call, c) => {
        lines += line({ type: 'assistant', uuid: `${tag}-a${t}-${c}`, timestamp: iso(at + 2000 + c * 3000), message: { id: `${tag}-m${t}-${c}`, role: 'assistant', content: [{ type: 'tool_use', id: call.id, name: 'Bash', input: { command: call.command } }], stop_reason: 'tool_use' } });
        lines += line({ type: 'user', uuid: `${tag}-o${t}-${c}`, timestamp: iso(at + 3000 + c * 3000), message: { role: 'user', content: [{ type: 'tool_result', tool_use_id: call.id, content: call.output }] } });
      });
      lines += line({ type: 'assistant', uuid: `${tag}-e${t}`, timestamp: iso(at + 90_000), message: { id: `${tag}-m${t}-end`, role: 'assistant', content: [{ type: 'text', text: answer }], stop_reason: 'end_turn' } });
      return lines;
    }).join('');
  },
  codex(tag, turns) {
    const line = (at, payload) => `${JSON.stringify({ timestamp: iso(at), type: 'event_msg', payload })}\n`;
    const item = (at, turn, value) => line(at, { type: 'item_completed', thread_id: tag, turn_id: turn, item: value, started_at_ms: at, completed_at_ms: at });
    return turns.map(({ t, at, ask, think, calls, answer }) => {
      const turn = `${tag}-turn${t}`;
      let lines = line(at, { type: 'task_started', turn_id: turn, started_at: Math.floor(at / 1000), model_context_window: 258400 });
      lines += item(at, turn, { type: 'UserMessage', id: `${tag}-u${t}`, content: [{ type: 'text', text: ask, text_elements: [] }] });
      lines += item(at + 1000, turn, { type: 'Reasoning', id: `${tag}-r${t}`, summary_text: [think], raw_content: [] });
      calls.forEach((call, c) => {
        lines += item(at + 2000 + c * 3000, turn, { type: 'CommandExecution', id: call.id, command: ['/bin/zsh', '-lc', call.command], source: 'unified_exec_startup', status: 'completed', aggregated_output: call.output, exit_code: 0, duration: { secs: 1, nanos: 0 } });
      });
      lines += item(at + 90_000, turn, { type: 'AgentMessage', id: `${tag}-e${t}`, content: [{ type: 'Text', text: answer }], phase: 'final_answer' });
      lines += line(at + 90_000, { type: 'task_complete', turn_id: turn, last_agent_message: answer, started_at: Math.floor(at / 1000), completed_at: Math.floor(at / 1000) + 90, duration_ms: 90_000 });
      return lines;
    }).join('');
  },
  opencode(tag, turns) {
    const line = (value) => `${JSON.stringify(value)}\n`;
    return turns.map(({ t, at, ask, think, calls, answer }) => {
      let lines = line({ id: `${tag}-u${t}`, type: 'user', text: ask, time: { created: at } });
      lines += line({ id: `${tag}-a${t}`, type: 'assistant', time: { created: at + 1000 }, content: [
        { type: 'reasoning', text: think, time: { created: at + 1000 } },
        ...calls.map((call, c) => ({ type: 'tool', id: call.id, name: 'shell', time: { created: at + 2000 + c * 3000, completed: at + 3000 + c * 3000 },
          state: { status: 'completed', input: { command: call.command }, content: [{ type: 'text', text: call.output }], metadata: { exit: 0 } } })),
        { type: 'text', text: answer, time: { created: at + 90_000 } },
      ] });
      lines += line({ id: `${tag}-i${t}`, type: 'idle', outcome: 'succeeded', time: { created: at + 90_000 } });
      return lines;
    }).join('');
  },
};
const fileOf = {
  claude: (tag) => join(homes.claude, 'projects', '-tmp-open-speed', `${tag}.jsonl`),
  codex: (tag) => join(homes.codex, 'sessions', '2026', '10', '20', `rollout-2026-10-20T09-00-00-${tag}.jsonl`),
  opencode: (tag) => join(homes.opencode, 'sessions', `${tag}.jsonl`),
};
const CLIS = ['claude', 'codex', 'opencode'];
// Per CLI: long (65 turns, 12 calls of ~6.5 KB: over 5 MB), short (4 turns), running (20 turns,
// grows while measured), cold (a long one the app doesn't read ahead).
const SHAPES = { long: [65, 12, 6500], short: [4, 2, 400], running: [20, 4, 1500], cold: [65, 12, 6500] };
const sessions = [];
for (const cli of CLIS) {
  for (const [kind, [turns, calls, bytes]] of Object.entries(SHAPES)) {
    const tag = `${cli}-${kind}`;
    const file = fileOf[cli](tag);
    mkdirSync(join(file, '..'), { recursive: true });
    writeFileSync(file, write[cli](tag, turnsOf(tag, turns, calls, bytes)));
    sessions.push({ cli, kind, tag, file, turns, bytes: statSync(file).size });
  }
}
/** Adds a finished turn to a running session's file, as its CLI would while it isn't shown. */
const grow = (session) => {
  session.grown = (session.grown ?? 0) + 1;
  const [turn] = turnsOf(session.tag, 1, 1, 300).map((t) => ({ ...t, t: session.turns + session.grown, at: BASE + 86_400_000 + session.grown * 1000 }));
  appendFileSync(session.file, write[session.cli](session.tag, [turn]));
};

// ---- The daemon: one connection, length-prefixed JSON frames.
const env = { ...process.env, HOME: home, CLAUDE_CONFIG_DIR: homes.claude, CODEX_HOME: homes.codex, OPENCODE_CONFIG_DIR: homes.opencode, OPENCODE_TEST_HOME: homes.opencode, XDG_DATA_HOME: join(homes.opencode, 'xdg') };
let daemon;
let link;
async function startDaemon() {
  const tokenPath = join(data, 'run/ipc.token');
  const socketPath = join(data, 'run/brigadierd.sock');
  rmSync(socketPath, { force: true });
  daemon = spawn(daemonBin, ['--data-dir', data, '--foreground'], { stdio: ['ignore', 'ignore', 'pipe'], env });
  daemon.log = '';
  daemon.stderr.on('data', (chunk) => { daemon.log += chunk; });
  for (let i = 0; i < 400 && !(existsSync(tokenPath) && existsSync(socketPath)); i++) await pause(25);
  const socket = createConnection(socketPath);
  await new Promise((resolve, reject) => { socket.once('connect', resolve); socket.once('error', reject); });
  const pending = new Map();
  const parts = new Map();
  let welcome;
  let onWelcome;
  const welcomed = new Promise((r) => { onWelcome = r; });
  let buffered = Buffer.alloc(0);
  link = { socket, pending, onFrame: () => {}, nextId: 0 };
  socket.on('data', (chunk) => {
    buffered = Buffer.concat([buffered, chunk]);
    while (buffered.length >= 4 && buffered.length >= 4 + buffered.readUInt32BE(0)) {
      const length = buffered.readUInt32BE(0);
      const frame = JSON.parse(buffered.subarray(4, 4 + length).toString());
      buffered = buffered.subarray(4 + length);
      if (frame.type === 'welcome') { welcome = frame; onWelcome(); } else if (frame.type === 'threadViewPart') {
        // An oversized thread's response comes in pieces of its JSON.
        const joined = (parts.get(frame.id) ?? '') + frame.json;
        if (!frame.last) parts.set(frame.id, joined);
        else { parts.delete(frame.id); const whole = JSON.parse(joined); pending.get(frame.id)?.(whole.result ?? whole); pending.delete(frame.id); }
      } else if (frame.type === 'response') {
        pending.get(frame.id)?.(frame.result);
        pending.delete(frame.id);
      } else link.onFrame(frame);
    }
  });
  send({ type: 'hello', token: readFileSync(tokenPath, 'utf8').trim(), protocol: 19, client: { name: 'open-speed-probe', pid: process.pid } });
  await welcomed;
  link.welcome = welcome;
}
function send(frame) {
  const body = Buffer.from(JSON.stringify(frame));
  const head = Buffer.alloc(4);
  head.writeUInt32BE(body.length);
  link.socket.write(Buffer.concat([head, body]));
}
const ask = (request) => new Promise((resolve) => {
  const id = ++link.nextId;
  link.pending.set(id, resolve);
  send({ type: 'request', id, request });
});
const must = async (request) => {
  const result = await ask(request);
  if (result.status !== 'ok') throw new Error(`${request.method}: ${JSON.stringify(result)}`);
  return result.value;
};
async function stopDaemon() {
  link.socket.destroy();
  const exited = new Promise((r) => daemon.once('exit', r));
  daemon.kill('SIGTERM');
  await exited;
}

await startDaemon();
const { catalog } = await must({ method: 'getCatalog' });
await must({ method: 'updateSettings', settings: { ...catalog.settings, onboarded: true, introSeen: true } });
// Oldest first: the cold ones and a filler fall outside the ten newest the app reads ahead.
const order = [...sessions.filter((s) => s.kind === 'cold'), { cli: 'claude', kind: 'filler', tag: 'filler' }, ...sessions.filter((s) => s.kind !== 'cold')];
for (const session of order) {
  const created = await must({ method: 'createConversation', kind: 'chat', projectId: null, title: session.tag, setup: null });
  session.id = created.conversation?.id ?? created.id;
  await pause(15);
  if (session.kind === 'filler') continue;
  // Its record, where a data dir under /tmp keeps it (`<data>-s/<last 8 of its id>`), naming its CLI session.
  const record = join(`${data}-s`, session.id.slice(-8), 'record');
  mkdirSync(record, { recursive: true });
  writeFileSync(join(record, 'lineage.json'), JSON.stringify([{ provider: session.cli, sessionId: session.tag, file: session.file, startedAt: BASE }]));
}
const byId = Object.fromEntries(sessions.map((s) => [s.id, s]));
await stopDaemon();

// ---- The page.
await build({ root, configFile: join(root, 'vite.config.ts'), logLevel: 'warn',
  build: { outDir, emptyOutDir: true, rolldownOptions: { input: { openSpeed: join(root, 'fixtures/open-speed.html') } } } });
const server = await preview({ root, configFile: join(root, 'vite.config.ts'), logLevel: 'warn', build: { outDir }, preview: { port: 0 } });
const browser = await chromium.launch({ headless: true, args: process.platform === 'darwin' ? ['--single-process'] : [], executablePath: process.env.CHROME_BIN });

// Every request the page makes, by method, into the switch under way; and what each threadView said.
let bucket = [];
const daemonCounts = {};
async function forward(request) {
  const asked = { method: request.method, at: now(), ms: NaN };
  bucket.push(asked);
  const result = await ask(request);
  asked.ms = now() - asked.at;
  if (result.status !== 'ok') return { error: result.error };
  if (request.method === 'threadView') daemonCounts[request.conversationId] = { entries: result.value.view.entries.length, turns: result.value.view.turns.length };
  return { value: result.value };
}

/** In the page: selects `id` and samples every frame until the thread has been whole for 30
 * frames. `scrollUp`: then scrolls it up, so its return shows whether its place was kept. */
function sample({ id, tag, rows, scrollUp }) {
  const viewport = () => document.querySelector('[data-slot="aui_thread-viewport"]');
  const place = (view) => ({ top: Math.round(view.scrollTop), gap: Math.round(view.scrollHeight - view.clientHeight - view.scrollTop) });
  const key = `open-speed-place:${id}`;
  const frame = () => new Promise((r) => requestAnimationFrame(r));
  return new Promise((resolve) => {
    const start = performance.now();
    const left = JSON.parse(sessionStorage.getItem(key) ?? 'null');
    const result = { full: null, blank: 0, loader: 0, frames: 0, moves: 0, rows: null, mounted: 0, dupRows: 0, left, back: null };
    let fullFrames = 0;
    let last = null;
    // A screen with no thread before (a new chat) is no blank thread.
    const hadRows = document.querySelectorAll('[data-thread-row]').length > 0;
    void window.openSpeed.select({ type: 'conversation', id });
    const tick = async () => {
      const at = performance.now() - start;
      result.frames++;
      const loading = !!document.querySelector('[data-slot="aui_thread-history-loading"]');
      const all = [...document.querySelectorAll('[data-thread-row]')];
      if (loading) result.loader++;
      if ((hadRows && all.length === 0) || loading) result.blank++;
      const view = viewport();
      // Its own rows: the old thread's stay up until the new one draws.
      const mine = window.openSpeed.selected() === id && all.some((row) => row.textContent.includes(tag));
      const size = Number(all[0]?.getAttribute('aria-setsize') ?? NaN);
      const covered = all.reduce((sum, row) => sum + row.getBoundingClientRect().height, 0);
      const whole = mine && view && (rows === null || size === rows) && (view.scrollHeight <= view.clientHeight || covered >= view.clientHeight * 0.9);
      if (whole) {
        const tops = new Map(all.map((row) => [row.getAttribute('data-thread-row'), Math.round(row.getBoundingClientRect().top)]));
        if (result.full === null) {
          result.full = at;
          result.rows = size;
          result.mounted = all.length;
          result.dupRows = all.length - new Set(all.map((row) => row.getAttribute('data-thread-row'))).size;
          result.back = place(view);
        } else if (last && [...tops].some(([row, top]) => last.has(row) && Math.abs(last.get(row) - top) > 1)) result.moves++;
        last = tops;
        fullFrames++;
      }
      if (fullFrames < 30 && at < 3000) return requestAnimationFrame(tick);
      const v = viewport();
      if (v && scrollUp) {
        v.scrollTop = Math.max(0, v.scrollTop - 3000);
        for (let i = 0; i < 10; i++) await frame();
      }
      if (v) sessionStorage.setItem(key, JSON.stringify(place(v)));
      // When the thread's answer reached the page: the rest of `full` is the page's own.
      const read = window.openSpeedCalls.find((call) => call.method === 'threadView' && call.asked >= start);
      result.threadAnswered = read ? read.answered - start : null;
      const held = window.openSpeed.views()[id];
      const ids = (held?.entries ?? []).map((entry) => entry.event.itemId).filter(Boolean);
      result.heldEntries = held?.entries.length ?? 0;
      result.heldTurns = held?.turns.length ?? 0;
      result.dupItems = ids.length - new Set(ids).size;
      resolve(result);
    };
    requestAnimationFrame(tick);
  });
}

const rowsOf = {};
const runs = [];
let page;
/** Opens the page, or loads it again (one page: headless Chromium in one process dies on a second). */
async function openPage() {
  if (!page) {
    page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
    page.on('pageerror', (error) => console.error('page error:', error.message));
    page.on('console', (message) => { if (message.type() === 'error') console.error('page console:', message.text()); });
    await page.exposeFunction('daemon', forward);
    await page.exposeFunction('subscribed', async () => {
      send({ type: 'request', id: ++link.nextId, request: { method: 'subscribe', afterSeq: link.welcome.lastSeq, metrics: false } });
      setTimeout(() => heard({ type: 'connected', daemon: link.welcome.daemon, lastSeq: link.welcome.lastSeq }), 0);
    });
  }
  link.onFrame = (frame) => {
    if (frame.type === 'event') heard({ type: 'event', event: frame.event });
    else if (frame.type === 'threadNews') heard({ type: 'threadNews', news: frame.news });
    else if (frame.type === 'lagged') heard({ type: 'lagged', resumeAfter: frame.resumeAfter });
  };
  await page.goto(`${server.resolvedUrls.local[0]}fixtures/open-speed.html`);
  await page.locator('[data-slot="sidebar-wrapper"]').first().waitFor({ timeout: 15000 });
  // Startup's reads and the read-ahead, then quiet.
  await settle();
}
function heard(event) {
  page?.evaluate((sent) => window.bridged?.(sent), event).catch(() => undefined);
}
async function settle() {
  await pause(1500);
  for (let quiet = 0; quiet < 10;) {
    const seen = bucket.length;
    await pause(100);
    quiet = bucket.length === seen ? quiet + 1 : 0;
  }
}

async function measure(phase, session, label = session.kind) {
  bucket = [];
  const result = await page.evaluate(sample, { id: session.id, tag: session.tag, rows: rowsOf[session.id] ?? null, scrollUp: session.kind === 'long' && label === 'first-open' });
  // Where it was left, against where it showed on return.
  result.kept = result.left && result.back ? Math.min(Math.abs(result.left.top - result.back.top), Math.abs(result.left.gap - result.back.gap)) <= 2 : null;
  const ipc = {};
  for (const { method } of bucket) ipc[method] = (ipc[method] ?? 0) + 1;
  // How long the daemon took to answer its thread and its records.
  for (const { method, ms } of bucket) if (method === 'threadView' || method === 'getConversation') result[`${method}Ms`] = Math.round(ms);
  if (result.full !== null && rowsOf[session.id] === undefined) rowsOf[session.id] = result.rows;
  runs.push({ phase, cli: session.cli, kind: label, ...result, ipc, daemon: daemonCounts[session.id] ?? null });
  await pause(250);
}

async function round(phase) {
  // What it reads after a restart may hold more (a running file grew).
  for (const id of Object.keys(rowsOf)) delete rowsOf[id];
  for (const cli of CLIS) await measure(`${phase}`, sessions.find((s) => s.cli === cli && s.kind === 'cold'), 'cold-not-read-ahead');
  for (const cli of CLIS) {
    const rotation = ['long', 'short', 'running'].map((kind) => sessions.find((s) => s.cli === cli && s.kind === kind));
    const running = rotation[2];
    const growing = setInterval(() => grow(running), 1000);
    const seen = new Set();
    for (let i = 0; i < switches; i++) {
      const session = rotation[i % 3];
      await measure(phase, session, seen.has(session.id) ? 'switch' : 'first-open');
      seen.add(session.id);
    }
    clearInterval(growing);
  }
}

try {
  await startDaemon();
  await openPage();
  await round('load');
  await stopDaemon();
  await startDaemon();
  await openPage();
  await round('restart');
  // What the daemon holds now, against what the page holds.
  bucket = [];
  for (const session of sessions) {
    const view = (await must({ method: 'threadView', conversationId: session.id })).view;
    const held = await page.evaluate((id) => { const v = window.openSpeed.views()[id]; return v ? { entries: v.entries.length, turns: v.turns.length } : null; }, session.id);
    session.final = { daemonEntries: view.entries.length, daemonTurns: view.turns.length, held };
  }
} finally {
  await browser.close();
  await server.close();
  if (link) await stopDaemon().catch(() => undefined);
}

const sorted = (values) => values.filter(Number.isFinite).toSorted((a, b) => a - b);
const median = (values) => { const s = sorted(values); return s.length ? s[Math.floor((s.length - 1) / 2)] : NaN; };
const max = (values) => sorted(values).at(-1) ?? NaN;
const ms = (value) => (Number.isFinite(value) ? `${Math.round(value)}` : '-');
console.log(`files: ${sessions.map((s) => `${s.tag} ${s.turns} turns ${(s.bytes / 1e6).toFixed(2)} MB`).join(', ')}`);
console.log('phase    cli       kind                 n  whole ms med/max  blank max  loader max  moves max  scroll kept  ipc (all switches)  missed  dup rows/items');
for (const phase of ['load', 'restart']) {
  for (const cli of CLIS) {
    for (const kind of ['cold-not-read-ahead', 'first-open', 'switch']) {
      const rows = runs.filter((r) => r.phase === phase && r.cli === cli && r.kind === kind);
      if (!rows.length) continue;
      const ipc = {};
      for (const r of rows) for (const [m, n] of Object.entries(r.ipc)) ipc[m] = (ipc[m] ?? 0) + n;
      const reads = rows.filter((r) => r.threadViewMs !== undefined).map((r) => `threadView ${r.threadViewMs} ms at the daemon link, in the page at ${Math.round(r.threadAnswered)} ms; getConversation ${r.getConversationMs} ms`);
      console.log(`${phase.padEnd(8)} ${cli.padEnd(9)} ${kind.padEnd(20)} ${String(rows.length).padStart(2)}  ${`${ms(median(rows.map((r) => r.full)))}/${ms(max(rows.map((r) => r.full)))}`.padStart(13)}  ${String(max(rows.map((r) => r.blank))).padStart(9)}  ${String(max(rows.map((r) => r.loader))).padStart(10)}  ${String(max(rows.map((r) => r.moves))).padStart(9)}  ${`${rows.filter((r) => r.kept === true).length}/${rows.filter((r) => r.kept !== null).length}`.padStart(11)}  ${JSON.stringify(ipc)}  ${rows.filter((r) => r.full === null).length}  ${max(rows.map((r) => r.dupRows))}/${max(rows.map((r) => r.dupItems))}${reads.length ? `  (${reads.join('; ')})` : ''}`);
    }
  }
}
console.log('held vs daemon at the end (entries/turns):');
for (const s of sessions) console.log(`  ${s.tag.padEnd(18)} daemon ${s.final.daemonEntries}/${s.final.daemonTurns}  page ${s.final.held ? `${s.final.held.entries}/${s.final.held.turns}` : 'not held'}${s.grown ? `  (file grew ${s.grown} turns)` : ''}`);
if (process.env.OPEN_SPEED_JSON) writeFileSync(process.env.OPEN_SPEED_JSON, JSON.stringify({ sessions, runs }, null, 1));
if (process.env.OPEN_SPEED_LOG) console.log(daemon.log);
