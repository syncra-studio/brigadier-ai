// Measures the app under a streaming load: a production build of the stream-load fixture (the
// whole app on a long session whose turn streams a delta at a time, other sessions working,
// then an approval waiting) in headless Chromium, with React's profiling build so its commits
// count. Per phase: frames and their gaps, long tasks, commits, and how often chosen components
// drew; and where the main thread's time went while it streamed (a CPU profile, by function).
// The draws are counted in a second run, as counting costs time of its own.
// With STREAM_LOAD_EXTRA=1 it goes on to typing (this script types into the composer), switching
// between two long sessions, showing the earlier turns and scrolling.
// CHROME_BIN=<a Chromium> node scripts/measure-stream-load.mjs [turns] [rate] [scratch dir]
import { spawn } from 'node:child_process';
import { existsSync, mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { build, preview } from 'vite';

const turns = Number(process.argv[2] ?? 150);
const rate = Number(process.argv[3] ?? 50);
const scratch = process.argv[4] ?? `/tmp/stream-load-${process.pid}`;
const extra = process.env.STREAM_LOAD_EXTRA === '1' ? '&extra=1' : '';
// STREAM_LOAD_FOCUS=<phase> profiles only that phase of the extra ones.
const focus = process.env.STREAM_LOAD_FOCUS ?? null;
const root = fileURLToPath(new URL('..', import.meta.url));
const outDir = join(scratch, 'dist');

rmSync(scratch, { recursive: true, force: true });
mkdirSync(scratch, { recursive: true });

await build({
  root, configFile: join(root, 'vite.config.ts'), logLevel: 'warn',
  // STREAM_LOAD_PLAIN=1 times React's own production build: no commits counted.
  resolve: { alias: process.env.STREAM_LOAD_PLAIN === '1' ? [] : [{ find: /^react-dom\/client$/, replacement: 'react-dom/profiling' }] },
  build: { outDir, emptyOutDir: true, minify: false, rolldownOptions: { input: { streamLoad: join(root, 'fixtures/stream-load.html') } } },
});
const server = await preview({ root, configFile: join(root, 'vite.config.ts'), logLevel: 'warn', build: { outDir }, preview: { port: 0 } });

function chromium() {
  if (process.env.CHROME_BIN) return process.env.CHROME_BIN;
  const cache = join(homedir(), 'Library/Caches/ms-playwright');
  for (const entry of existsSync(cache) ? readdirSync(cache) : []) {
    for (const binary of ['chrome-headless-shell-mac-arm64/chrome-headless-shell', 'chrome-headless-shell-linux64/chrome-headless-shell']) {
      if (entry.startsWith('chromium_headless_shell') && existsSync(join(cache, entry, binary))) return join(cache, entry, binary);
    }
  }
  throw new Error('Set CHROME_BIN to a Chromium.');
}

/** A profile node's function, file and line. */
const nameOf = (node) => `${node.callFrame.functionName || '(anonymous)'} ${node.callFrame.url.split('/').pop()}:${node.callFrame.lineNumber + 1}`;

/** The `n` costliest functions in `map`, leaving out the profiler's own buckets. */
const top = (map, n) => [...map].filter(([key]) => !/^\((root|program|idle|garbage collector)\)/.test(key)).toSorted((a, b) => b[1] - a[1]).slice(0, n).map(([key, ms]) => `${Math.round(ms)} ms  ${key}`);

/** Where the time went while it streamed: inclusive and self milliseconds by function. */
function byFunction(cpu) {
  const nodes = new Map(cpu.nodes.map((node) => [node.id, node]));
  const self = new Map();
  for (let i = 0; i < cpu.samples.length; i++) self.set(cpu.samples[i], (self.get(cpu.samples[i]) ?? 0) + (cpu.timeDeltas[i + 1] ?? 0) / 1000);
  const inclusive = new Map();
  const selfBy = new Map();
  const walk = (nodeId, stack) => {
    const node = nodes.get(nodeId);
    const key = nameOf(node);
    const fresh = !stack.has(key);
    if (fresh) stack.add(key);
    const ms = self.get(nodeId) ?? 0;
    selfBy.set(key, (selfBy.get(key) ?? 0) + ms);
    for (const each of stack) inclusive.set(each, (inclusive.get(each) ?? 0) + ms);
    for (const child of node.children ?? []) walk(child, stack);
    if (fresh) stack.delete(key);
  };
  walk(cpu.nodes[0].id, new Set());
  const total = [...self].reduce((sum, [nodeId, ms]) => (/^\((idle|program)\)$/.test(nodes.get(nodeId).callFrame.functionName) ? sum : sum + ms), 0);
  return { busyMs: Math.round(total), inclusive: top(inclusive, 40), self: top(selfBy, 25) };
}

/** One run of the fixture with `query`: its result, and a CPU profile of its streaming when `profiled`. */
async function run(query, profiled) {
  const profile = join(scratch, `profile-${profiled ? 'timed' : 'counted'}`);
  const browser = spawn(chromium(), ['--headless', '--no-sandbox', '--hide-scrollbars', '--window-size=1280,976', '--remote-debugging-port=0', `--user-data-dir=${profile}`, 'about:blank'], { stdio: 'ignore', detached: true });
  try {
    let port = 0;
    for (let i = 0; i < 200 && !port; i++) {
      try { port = Number(readFileSync(join(profile, 'DevToolsActivePort'), 'utf8').split('\n')[0]); } catch { await new Promise((r) => setTimeout(r, 100)); }
    }
    const target = (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json()).find((each) => each.type === 'page');
    const socket = new WebSocket(target.webSocketDebuggerUrl);
    await new Promise((resolve) => socket.addEventListener('open', resolve, { once: true }));
    let id = 0;
    const replies = new Map();
    // An error the page throws ends the run at once, naming it.
    // Set by the socket's listener, so the wait loop below sees it change.
    const page = { thrown: null };
    socket.addEventListener('message', (event) => {
      const message = JSON.parse(String(event.data));
      if (message.id !== undefined) replies.get(message.id)?.(message);
      if (message.method === 'Runtime.exceptionThrown') page.thrown ??= message.params.exceptionDetails.exception?.description ?? message.params.exceptionDetails.text;
    });
    const call = (method, params = {}) => new Promise((resolve, reject) => {
      const asked = ++id;
      replies.set(asked, (message) => (message.error ? reject(new Error(`${method}: ${message.error.message}`)) : resolve(message.result)));
      socket.send(JSON.stringify({ id: asked, method, params }));
    });
    const evaluate = async (expression) => (await call('Runtime.evaluate', { expression, returnByValue: true })).result.value;
    await call('Runtime.enable');
    await call('Emulation.setFocusEmulationEnabled', { enabled: true });
    await call('Profiler.enable');
    await call('Profiler.setSamplingInterval', { interval: 200 });
    await call('Page.navigate', { url: `${server.resolvedUrls.local[0]}fixtures/stream-load.html?drive=1&turns=${turns}&rate=${rate}${extra}&${query}` });
    let result = null;
    let profiling = false;
    let cpu = null;
    const deadline = Date.now() + 180000;
    while (!result && !page.thrown && Date.now() < deadline) {
      // The CPU profile covers both streaming phases.
      if (profiled && !profiling && (await evaluate(focus ? `document.documentElement.dataset.phase === ${JSON.stringify(focus)}` : '"streaming" in document.documentElement.dataset'))) {
        profiling = true;
        await call('Profiler.start');
      }
      if (focus && profiling && !cpu && (await evaluate(`document.documentElement.dataset.phase !== ${JSON.stringify(focus)}`))) cpu = (await call('Profiler.stop')).profile;
      if (await evaluate('"typing" in document.documentElement.dataset && !("typed" in document.documentElement.dataset)')) {
        // Ten keys a second, as a quick typist.
        for (let key = 0; key < 30; key++) {
          const letter = String.fromCodePoint(97 + (key % 26));
          await call('Input.dispatchKeyEvent', { type: 'keyDown', key: letter, code: `Key${letter.toUpperCase()}`, text: letter, windowsVirtualKeyCode: 65 + (key % 26) });
          await call('Input.dispatchKeyEvent', { type: 'keyUp', key: letter, code: `Key${letter.toUpperCase()}`, windowsVirtualKeyCode: 65 + (key % 26) });
          await new Promise((r) => setTimeout(r, 100));
        }
        await evaluate('document.documentElement.dataset.typed = ""');
      }
      result = await evaluate('document.getElementById("stream-load-result")?.textContent ?? null');
      if (!result) await new Promise((r) => setTimeout(r, focus ? 20 : 200));
    }
    if (profiling && !cpu) cpu = (await call('Profiler.stop')).profile;
    socket.close();
    if (!result) throw new Error(page.thrown ?? 'the fixture wrote no result');
    return { result: JSON.parse(result), cpu };
  } finally {
    process.kill(-browser.pid, 'SIGKILL');
  }
}

try {
  const timed = await run('count=0', true);
  const counted = await run('count=1', false);
  for (const [phase, seen] of Object.entries(counted.result)) if (seen?.renders) timed.result[phase].renders = seen.renders;
  console.log(JSON.stringify(timed.result, null, 2));
  if (timed.cpu) console.log(JSON.stringify(byFunction(timed.cpu), null, 2));
  // STREAM_LOAD_PROFILE=<file> keeps the CPU profile, for DevTools.
  if (timed.cpu && process.env.STREAM_LOAD_PROFILE) writeFileSync(process.env.STREAM_LOAD_PROFILE, JSON.stringify(timed.cpu));
} finally {
  await new Promise((resolve) => server.httpServer.close(resolve));
  // STREAM_LOAD_KEEP=1 keeps the build, to read the profile's lines against.
  if (process.env.STREAM_LOAD_KEEP !== '1') rmSync(scratch, { recursive: true, force: true });
}
