// Measures how long Settings takes to show, frame by frame from the click, in a production build
// of the settings-open fixture with IPC answered after a daemon-like delay per method (see the
// fixture's `delayOf`). Per page:
//   open    a click that opens Settings at the page (the Settings button for General), on a
//           fresh load left idle for a moment first: click → navigation and the page's layout
//           painted, click → the page's data painted;
//   switch  a click on the page in the navigation, its first visit: the same two;
//   revisit the same click again later, after every page was shown once.
// Also counted per click: frames with the page area blank after the click, frames showing a
// spinner or a loading line, and whether the page's height changed once its layout showed.
// PLAYWRIGHT_MODULE=<installed module> CHROME_BIN=<installed browser> node scripts/measure-settings-open.mjs [runs] [idle ms]
import { rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { build, preview } from 'vite';

const playwright = await import(process.env.PLAYWRIGHT_MODULE || 'playwright');
const { chromium } = playwright.chromium ? playwright : playwright.default;
const runs = Number(process.argv[2] ?? 5);
const idleMs = Number(process.argv[3] ?? 2500);
const root = fileURLToPath(new URL('..', import.meta.url));
const outDir = join(tmpdir(), `settings-open-${process.pid}`);

// Each page: its nav label, its title, and a text its data shows once read (none: the page has
// nothing to read, its layout is all).
const PAGES = [
  { id: 'general', label: 'General', title: 'General', data: 'Open at login' },
  { id: 'voice', label: 'Voice', title: 'Voice', data: null },
  { id: 'configuration', label: 'Configuration', title: 'Configuration', data: '2 saved' },
  { id: 'shortcuts', label: 'Keyboard shortcuts', title: 'Keyboard shortcuts', data: null },
  { id: 'providers', label: 'Providers', title: 'Providers', data: 'Most capable' },
  { id: 'usage', label: 'Usage', title: 'Usage', data: '% left' },
  { id: 'git', label: 'Git', title: 'Git', data: null },
  { id: 'computerUse', label: 'Computer use', title: 'Computer use', data: 'Finish setup' },
  { id: 'browser', label: 'Browser', title: 'Browser', data: null },
  { id: 'browserImport', label: 'Import Browser Data', title: 'Import Browser Data', data: 'Firefox' },
  { id: 'storage', label: 'Storage', title: 'Storage', data: 'used by Brigadier' },
  { id: 'inspector', label: 'Inspector', title: 'Inspector', data: null },
  { id: 'archived', label: 'Archived chats', title: 'Archived chats', data: 'Checkout page redesign' },
];
const SAMPLE_MS = 1600;

await build({ root, configFile: join(root, 'vite.config.ts'), logLevel: 'warn',
  build: { outDir, emptyOutDir: true, rolldownOptions: { input: { settingsOpen: join(root, 'fixtures/settings-open.html') } } } });
const server = await preview({ root, configFile: join(root, 'vite.config.ts'), logLevel: 'warn', build: { outDir }, preview: { port: 0 } });
const url = server.resolvedUrls.local[0];
const browser = await chromium.launch({ headless: true, args: process.platform === 'darwin' ? ['--single-process'] : [], executablePath: process.env.CHROME_BIN });

/** In the page: clicks `target` and samples every frame for `ms`. */
function sample({ target, title, data, ms }) {
  return new Promise((resolve) => {
    const button = target.startsWith('nav:')
      ? [...document.querySelectorAll('[data-slot="sidebar-panel"] button')].find((b) => b.textContent.trim() === target.slice(4))
      : document.querySelector(target);
    if (!button) throw new Error(`no ${target}`);
    const frames = [];
    const start = performance.now();
    button.click();
    const tick = () => {
      const panel = document.querySelector('[data-slot="sidebar-panel"]');
      const main = document.querySelector('main');
      const page = main?.querySelector('[data-slot="settings-page"]');
      const text = main?.textContent ?? '';
      frames.push({
        t: performance.now() - start,
        nav: Boolean(panel?.textContent?.includes('Back to app')),
        layout: main?.querySelector('h1')?.textContent === title,
        data: data === null ? main?.querySelector('h1')?.textContent === title : text.includes(data),
        blank: !page,
        loading: Boolean(main?.querySelector('.animate-spin')) || /Loading…|Reading usage…|Looking at what/.test(text),
        // The page's column (under its drag band): its height as laid out.
        height: page?.firstElementChild?.children[1]?.getBoundingClientRect().height ?? 0,
      });
      if (performance.now() - start < ms) requestAnimationFrame(tick);
      else resolve(frames);
    };
    requestAnimationFrame(tick);
  });
}

function summary(frames) {
  const at = (test) => Math.round(frames.find(test)?.t ?? NaN);
  const shownAt = frames.findIndex((f) => f.nav && f.layout);
  return {
    layout: at((f) => f.nav && f.layout),
    data: at((f) => f.nav && f.layout && f.data),
    blankFrames: frames.filter((f) => f.blank).length,
    loadingFrames: frames.filter((f) => f.loading).length,
    // The page's height moved after its layout showed (data filling in pushed things around).
    jumped: shownAt >= 0 && frames.slice(shownAt).some((f) => Math.abs(f.height - frames[shownAt].height) > 1) ? 1 : 0,
  };
}

const results = [];
try {
  // One page throughout: a single-process browser goes with its last page.
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
  const fresh = async () => {
    await page.goto(`${url}fixtures/settings-open.html`);
    await page.locator('button[aria-label="Settings"]').waitFor();
    await page.waitForTimeout(idleMs);
  };
  for (let run = 0; run < runs; run++) {
    // Opening Settings at each page, from a fresh load.
    for (const p of PAGES) {
      await fresh();
      if (process.env.DEBUG) console.error('open', p.id);
      const target = p.id === 'general' ? 'button[aria-label="Settings"]' : `[data-open-page="${p.id}"]`;
      const frames = await page.evaluate(sample, { target, title: p.title, data: p.data, ms: SAMPLE_MS });
      results.push({ run, phase: 'open', page: p.id, ...summary(frames) });
      if (process.env.DEBUG && !frames.at(-1).data) console.error(p.id, 'never showed its data:', await page.locator('main').innerText());
    }
    // Moving between the pages: each one's first visit, then again.
    await fresh();
    await page.evaluate(sample, { target: 'button[aria-label="Settings"]', title: 'General', data: 'Open at login', ms: SAMPLE_MS });
    for (const phase of ['switch', 'revisit']) {
      for (const p of phase === 'switch' ? PAGES.slice(1) : PAGES) {
        const frames = await page.evaluate(sample, { target: `nav:${p.label}`, title: p.title, data: p.data, ms: SAMPLE_MS });
        results.push({ run, phase, page: p.id, ...summary(frames) });
      }
    }
  }
} finally {
  await browser.close();
  await server.close();
  rmSync(outDir, { recursive: true, force: true });
}

const median = (values) => values.toSorted((a, b) => a - b)[Math.floor(values.length / 2)];
console.log(`${runs} runs, ${idleMs} ms idle after load; medians in ms from the click (layout: navigation and the page's layout painted; data: its data painted too)`);
console.log('page            | open layout  data | switch layout  data | revisit layout  data | blank frames o/s/r | loading frames o/s/r | height moved o/s/r');
for (const p of PAGES) {
  const of = (phase) => results.filter((r) => r.phase === phase && r.page === p.id);
  const cell = (phase, key) => {
    const rows = of(phase);
    return rows.length ? String(median(rows.map((r) => r[key]))) : '-';
  };
  const three = (key) => ['open', 'switch', 'revisit'].map((phase) => cell(phase, key)).join('/');
  console.log(`${p.id.padEnd(15)} | ${cell('open', 'layout').padStart(11)} ${cell('open', 'data').padStart(5)} | ${cell('switch', 'layout').padStart(13)} ${cell('switch', 'data').padStart(5)} | ${cell('revisit', 'layout').padStart(14)} ${cell('revisit', 'data').padStart(5)} | ${three('blankFrames').padStart(18)} | ${three('loadingFrames').padStart(20)} | ${three('jumped').padStart(18)}`);
}
