// Measures Search on the most the daemon sends (2,000 files, 20,000 lines) in a production
// build of the search-scale fixture: how long the streamed results take to show in full and
// how long the page's main thread was blocked meanwhile, how long the view takes to open again
// on them, and the frame times while scrolling through them.
// PLAYWRIGHT_MODULE=<installed module> CHROME_BIN=<installed browser> node scripts/measure-search-scale.mjs [runs]
import { rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { build, preview } from 'vite';

const playwright = await import(process.env.PLAYWRIGHT_MODULE || 'playwright');
const { chromium } = playwright.chromium ? playwright : playwright.default;
const runs = Number(process.argv[2] ?? 3);
const root = fileURLToPath(new URL('..', import.meta.url));
const outDir = join(tmpdir(), `search-scale-${process.pid}`);

await build({ root, configFile: join(root, 'vite.config.ts'), logLevel: 'warn',
  build: { outDir, emptyOutDir: true, rolldownOptions: { input: { searchScale: join(root, 'fixtures/search-scale.html') } } } });
const server = await preview({ root, configFile: join(root, 'vite.config.ts'), logLevel: 'warn', build: { outDir }, preview: { port: 0 } });
const browser = await chromium.launch({ headless: true, args: process.platform === 'darwin' ? ['--single-process'] : [], executablePath: process.env.CHROME_BIN });

/** In the page: types the query and waits for every result; the time and the blocked time. */
function search() {
  return new Promise((resolve) => {
    const blocked = [];
    new PerformanceObserver((list) => { for (const entry of list.getEntries()) blocked.push(entry.duration); })
      .observe({ type: 'longtask' });
    const input = document.querySelector('input[aria-label="Search"]');
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(input, 'needle');
    const start = performance.now();
    input.dispatchEvent(new Event('input', { bubbles: true }));
    const tick = () => {
      const summary = document.querySelector('[data-slot="search-summary"]')?.textContent ?? '';
      if (summary === '20,000 results in 2,000 files') {
        resolve({ done: performance.now() - start, longTasks: blocked.length, blocked: blocked.reduce((a, b) => a + b, 0), longest: Math.max(0, ...blocked), rows: document.querySelectorAll('[data-panel="search"] button').length });
      } else requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  });
}

/** In the page: takes the view away and shows it again; the time until its results paint. */
function reopen() {
  return new Promise((resolve) => {
    window.showSearch(false);
    setTimeout(() => {
      const start = performance.now();
      window.showSearch(true);
      const tick = () => {
        if (document.querySelector('[data-panel="search"] mark')) resolve(performance.now() - start);
        else requestAnimationFrame(tick);
      };
      requestAnimationFrame(tick);
    }, 200);
  });
}

/** In the page: scrolls the results 60 px a frame for 3 s; the gaps between frames. */
function scroll() {
  return new Promise((resolve) => {
    const list = [...document.querySelectorAll('[data-panel="search"] *')].find((node) => {
      const style = getComputedStyle(node);
      return /(auto|scroll)/.test(style.overflowY) && node.scrollHeight > node.clientHeight;
    });
    const gaps = [];
    let last = performance.now();
    const start = last;
    const tick = (now) => {
      gaps.push(now - last);
      last = now;
      list.scrollTop += 60;
      if (now - start < 3000) requestAnimationFrame(tick);
      else resolve({ gaps, height: list.scrollHeight });
    };
    requestAnimationFrame(tick);
  });
}

const rows = [];
try {
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
  for (let run = 0; run < runs; run++) {
    await page.goto(`${server.resolvedUrls.local[0]}fixtures/search-scale.html`);
    await page.locator('input[aria-label="Search"]').waitFor();
    const found = await page.evaluate(search);
    await page.waitForTimeout(500);
    const opened = await page.evaluate(reopen);
    await page.waitForTimeout(500);
    const { gaps } = await page.evaluate(scroll);
    const sorted = gaps.toSorted((a, b) => a - b);
    rows.push({ ...found, opened, frames: gaps.length, frameMedian: sorted[Math.floor(sorted.length / 2)], frameP95: sorted[Math.floor(sorted.length * 0.95)], frameMax: sorted.at(-1), slowFrames: gaps.filter((gap) => gap > 50).length });
  }
} finally {
  await browser.close();
  await server.close();
  rmSync(outDir, { recursive: true, force: true });
}

const median = (key) => { const values = rows.map((row) => row[key]).toSorted((a, b) => a - b); return Math.round(values[Math.floor(values.length / 2)]); };
console.log(`2,000 files / 20,000 lines, ${runs} runs, medians:`);
console.log(`all shown ${median('done')} ms; main thread blocked ${median('blocked')} ms in ${median('longTasks')} long tasks (longest ${median('longest')} ms); ${median('rows')} buttons in the DOM`);
console.log(`opened again on them: ${median('opened')} ms to the first painted match`);
console.log(`scrolling 3 s: ${median('frames')} frames, gap median ${median('frameMedian')} ms, p95 ${median('frameP95')} ms, max ${median('frameMax')} ms, ${median('slowFrames')} over 50 ms`);
