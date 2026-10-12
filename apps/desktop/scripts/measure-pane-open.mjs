// Measures how long the right sidebar takes to show Files, Source or a cached Search, frame by frame from the key
// that opens it on that view, in a production build of the pane-open fixture with IPC answered
// after a daemon-like delay.
// PLAYWRIGHT_MODULE=<installed module> CHROME_BIN=<installed browser> node scripts/measure-pane-open.mjs [delay ms] [runs]
import { rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { build, preview } from 'vite';

const playwright = await import(process.env.PLAYWRIGHT_MODULE || 'playwright');
const { chromium } = playwright.chromium ? playwright : playwright.default;
const delay = Number(process.argv[2] ?? 25);
const runs = Number(process.argv[3] ?? 5);
const root = fileURLToPath(new URL('..', import.meta.url));
const outDir = join(tmpdir(), `shell-tabs-sidebar-pane-open-${process.pid}`);
// The column whose contents are measured, and its views, each with the key that opens the
// sidebar on it and a text its contents show once read.
const COLUMN = '[data-slot="workspace-pane"]';
const VIEWS = [
  { label: 'Files', key: { code: 'KeyE', metaKey: true, shiftKey: true }, text: 'apps' },
  { label: 'Source', key: { code: 'KeyG', ctrlKey: true, shiftKey: true }, text: 'file-0.ts' },
  { label: 'Search', key: { code: 'KeyF', metaKey: true, shiftKey: true }, text: 'needle in file-0.ts' },
];
const TOGGLE = { code: 'KeyB', metaKey: true, altKey: true };

await build({ root, configFile: join(root, 'vite.config.ts'), logLevel: 'warn',
  build: { outDir, emptyOutDir: true, rolldownOptions: { input: { paneOpen: join(root, 'fixtures/pane-open.html') } } } });
const server = await preview({ root, configFile: join(root, 'vite.config.ts'), logLevel: 'warn', build: { outDir }, preview: { port: 0 } });
const url = server.resolvedUrls.local[0];
const browser = await chromium.launch({ headless: true, args: process.platform === 'darwin' ? ['--single-process'] : [], executablePath: process.env.CHROME_BIN });

/** In the page: presses the view's key and samples every frame for a second. */
function sample({ column, key, text }) {
  return new Promise((resolve) => {
    const frames = [];
    const start = performance.now();
    window.dispatchEvent(new KeyboardEvent('keydown', { ...key, bubbles: true, cancelable: true }));
    const tick = () => {
      const element = document.querySelector(column);
      const rect = element?.getBoundingClientRect();
      const width = rect ? Math.max(0, Math.min(innerWidth, rect.right) - Math.max(0, rect.left)) : 0;
      // Its contents' opacity as drawn: the product of their own and every ancestor's.
      let opacity = element ? 1 : 0;
      for (let node = element?.querySelector('[data-pane]') ?? element; node instanceof Element; node = node.parentElement) opacity *= Number(getComputedStyle(node).opacity);
      frames.push({ t: performance.now() - start, present: Boolean(element?.textContent?.includes(text)), opacity, width });
      if (performance.now() - start < 1000) requestAnimationFrame(tick);
      else resolve(frames);
    };
    requestAnimationFrame(tick);
  });
}

/** When the contents first exist, first read as shown (half opaque, a usable width), and settle. */
function summary(frames) {
  const final = frames.at(-1).width;
  const at = (test) => Math.round(frames.find(test)?.t ?? NaN);
  return {
    present: at((f) => f.present),
    firstFrameHasContent: frames[0].present,
    emptyContentFrames: frames.filter((f) => !f.present).length,
    shown: at((f) => f.present && f.opacity >= 0.5 && f.width >= 40),
    settled: at((f) => f.present && f.opacity >= 0.99 && Math.abs(f.width - final) < 1),
    blankFrames: frames.findIndex((f) => f.present && f.opacity >= 0.5 && f.width >= 40),
  };
}

const results = [];
try {
  // One page throughout: a single-process browser goes with its last page.
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
  for (let run = 0; run < runs; run++) {
    await page.goto(`${url}fixtures/pane-open.html?delay=${delay}`);
    await page.evaluate(() => localStorage.clear());
    await page.reload();
    await page.locator('header[data-slot="test-toolbar"] button[aria-label="Right sidebar"]').waitFor();
    await page.waitForTimeout(300);
    for (const [phase, views] of [['cold', VIEWS], ['warm', VIEWS]]) {
      for (const view of views) {
        const frames = await page.evaluate(sample, { column: COLUMN, ...view });
        results.push({ run, phase, view: view.label, ...summary(frames) });
        // Closed again, its slide and the contents' removal over.
        await page.evaluate((key) => window.dispatchEvent(new KeyboardEvent('keydown', { ...key, bubbles: true, cancelable: true })), TOGGLE);
        await page.waitForTimeout(600);
      }
    }
  }
} finally {
  await browser.close();
  await server.close();
  rmSync(outDir, { recursive: true, force: true });
}

const median = (values) => values.toSorted((a, b) => a - b)[Math.floor(values.length / 2)];
console.log(`IPC delay ${delay} ms, ${runs} runs, medians in ms from the click:`);
for (const phase of ['cold', 'warm']) for (const { label } of VIEWS) {
  const rows = results.filter((r) => r.phase === phase && r.view === label);
  console.log(`${phase.padEnd(4)} ${label.padEnd(6)} present ${median(rows.map((r) => r.present))}  shown ${median(rows.map((r) => r.shown))}  settled ${median(rows.map((r) => r.settled))}  slide frames ${median(rows.map((r) => r.blankFrames))}  first frame content ${rows.every((r) => r.firstFrameHasContent)}  empty content frames ${median(rows.map((r) => r.emptyContentFrames))}`);
}
