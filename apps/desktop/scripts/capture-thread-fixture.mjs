// Captures the production thread fixture with an installed Chromium and Playwright.
// Run against Vite: PLAYWRIGHT_MODULE=<module path> CHROME_BIN=<binary> node scripts/capture-thread-fixture.mjs <base-url> <output-directory>.
import assert from "node:assert/strict";
import { mkdir, writeFile } from "node:fs/promises";
import { resolve } from "node:path";

const { chromium } = await import(
  process.env.PLAYWRIGHT_MODULE || "playwright"
);
const base = process.argv[2] || "http://127.0.0.1:15434";
const output = resolve(process.argv[3] || "thread-shots");
await mkdir(output, { recursive: true });
const browser = await chromium.launch({
  executablePath: process.env.CHROME_BIN,
  headless: true,
});
const errors = [];
const page = await browser.newPage({
  viewport: { width: 1280, height: 834 },
  deviceScaleFactor: 1,
  reducedMotion: "reduce",
});
page.on("pageerror", (error) => errors.push(error.message));
const metrics = [];
const url = (query) =>
  `${base}/fixtures/thread-replica.html?summary=0&approvals=0&${query}`;
const shot = async (name) => {
  const viewport = page.locator('[data-slot="aui_thread-viewport"]');
  if (await viewport.count())
    await viewport.evaluate((element) => {
      element.scrollTop = -element.scrollHeight;
    });
  await page.screenshot({ path: `${output}/${name}.png`, animations: "disabled" });
  const text = await page.locator("main").innerText();
  assert.doesNotMatch(
    text,
    /Used a tool|Using a tool|\btask-\d+\b|mcp__|functions\./,
  );
  const measured = await page
    .locator(
      '[data-slot="orchestrator-step"], [data-slot="worker-action"] > button',
    )
    .evaluateAll((rows) =>
      rows.map((row) => {
        const style = getComputedStyle(row);
        return {
          text: row.textContent,
          width: row.getBoundingClientRect().width,
          height: row.getBoundingClientRect().height,
          fontSize: style.fontSize,
          lineHeight: style.lineHeight,
          gap: style.gap,
          color: style.color,
        };
      }),
    );
  metrics.push({ name, rows: measured, text });
  assert.equal(
    await page.evaluate(
      () => document.documentElement.scrollWidth > innerWidth,
    ),
    false,
    "No viewport overflow",
  );
};
const load = async (query) => {
  await page.goto(url(query));
  await page.locator('[data-slot="request-work-header"]').waitFor();
};
try {
  await load("view=running");
  await shot("01-running");
  await page.locator('[data-slot="work-group"] > button').first().click();
  assert.match(
    await page.locator("main").innerText(),
    /Searched project memory/,
  );
  assert.match(await page.locator("main").innerText(), /Searching code/);
  await shot("02-running-actions");

  await load("view=done");
  await shot("03-done-collapsed");
  const divider = page.locator('[data-slot="request-work-header"]');
  await divider.focus();
  await page.keyboard.press("Enter");
  await page.locator('[data-slot="request-fold"]').waitFor();
  await shot("04-done-level1");
  for (const group of await page
    .locator('[data-slot="work-group"] > button')
    .all())
    await group.click();
  await shot("05-done-level2");
  const created = page.locator('[data-kind="created"] [role="button"]').first();
  await created.focus();
  await page.keyboard.press("Enter");
  assert.equal(await created.getAttribute("aria-expanded"), "true");
  await shot("06-done-level3");
  const thought = page
    .locator('[data-slot="thinking-settled"] > button')
    .first();
  await thought.click();
  await shot("07-thought-details");

  await load("view=running&worker=1");
  await page.locator('[data-slot="task-row"]').first().click();
  const worker = page.locator('[data-slot="worker-thread"]');
  await worker.waitFor();
  await worker.evaluate((element) => {
    element.scrollTop = 0;
  });
  await shot("08-worker");
  const groups = worker.locator('[data-slot="collapsible"] > button');
  for (const group of await groups.all()) {
    if ((await group.innerText()).includes("Read a file")) {
      await group.click();
      break;
    }
  }
  const web = worker
    .locator("button")
    .filter({
      hasText: "Searched the web for radix switch prefers-color-scheme",
    });
  await web.click();
  await worker.evaluate((element) => {
    element.scrollTop = 0;
  });
  assert.equal(await worker.locator('[data-slot="web-search"] li').count(), 3);
  await shot("09-web-results");
  const failed = worker
    .locator("button")
    .filter({ hasText: "Ran pnpm test settings" })
    .first();
  // It lives under the second action group, so expand that group first.
  if (!(await failed.isVisible())) {
    for (const group of await worker
      .locator('[data-slot="collapsible"] > button')
      .all()) {
      if ((await group.innerText()).includes("Edited")) await group.click();
    }
  }
  await failed.click();
  await failed.scrollIntoViewIfNeeded();
  assert.match(await worker.innerText(), /Exit code 1/);
  await shot("10-command-output");

  await load("view=running&thinking=1");
  assert.match(
    await page.locator('[data-slot="thinking-live"]').innerText(),
    /remaining tests/,
  );
  await shot("11-thinking-live");
  await page.setViewportSize({ width: 720, height: 834 });
  await shot("12-narrow");
  assert.deepEqual(errors, []);
  await writeFile(
    `${output}/measurements.json`,
    JSON.stringify(metrics, null, 2),
  );
  console.log(
    `Saved ${metrics.length} captures; copy, keyboard disclosures, results, output and overflow checks passed.`,
  );
} finally {
  await browser.close();
}
