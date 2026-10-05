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
      '[data-slot="orchestrator-step"]',
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
  await page.locator(".aui-md").first().waitFor();
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
  assert.match(await page.locator("main").innerText(), /Read apps\/desktop\/src\/state\/settings.ts/);
  assert.match(await page.locator("main").innerText(), /Ran pnpm typecheck/);
  // Worker lifecycle rows retain the existing size, spacing and live activity subrow.
  assert.match(await page.locator('[data-kind="created"] [role="button"]').first().getAttribute("class"), /min-h-row-sm.*gap-2/);
  assert.match(await page.locator("main").innerText(), /Running pnpm test settings/);
  await shot("02-running-actions");

  await load("view=done");
  const reply = page.locator('[data-slot="aui_assistant-message-content"]').last();
  await reply.locator("strong").filter({ hasText: "Theme" }).waitFor();
  assert.equal(await reply.locator("ul > li").count(), 3);
  assert.equal(await reply.locator("code").count(), 4);
  assert.doesNotMatch(await reply.innerText(), /\*\*Theme\*\*|^- Phase/m);
  await shot("03-done-collapsed");
  const divider = page.locator('[data-slot="request-work-header"]');
  await divider.click();
  assert.equal(await divider.evaluate((element) => element.matches(":focus-visible")), false);
  assert.equal(await divider.evaluate((element) => getComputedStyle(element).outlineStyle), "none");
  assert.equal(await divider.evaluate((element) => getComputedStyle(element).boxShadow), "none");
  // Keyboard users keep a visible focus indicator and can toggle with Enter.
  await page.keyboard.press("Tab");
  await page.keyboard.press("Shift+Tab");
  assert.equal(await divider.evaluate((element) => element.matches(":focus-visible")), true);
  assert.notEqual(await divider.evaluate((element) => getComputedStyle(element).boxShadow), "none");
  await page.keyboard.press("Enter");
  assert.equal(await divider.getAttribute("aria-expanded"), "false");
  await page.keyboard.press("Enter");
  assert.equal(await divider.getAttribute("aria-expanded"), "true");
  // Reset keyboard focus before exercising pointer interaction for review captures.
  await page.locator(".aui-md p").last().click();
  await divider.click();
  await divider.click();
  assert.equal(await divider.evaluate((element) => element.matches(":focus-visible")), false);
  assert.equal(await divider.evaluate((element) => getComputedStyle(element).outlineStyle), "none");
  assert.equal(await divider.evaluate((element) => getComputedStyle(element).boxShadow), "none");
  await page.locator('[data-slot="request-fold"]').waitFor();
  await shot("04-done-level1");
  for (const group of await page
    .locator('[data-slot="work-group"] > button')
    .all())
    await group.click();
  await shot("05-done-level2");
  const web = page.locator('[data-kind="searchedWeb"] [role="button"]').first();
  await web.click();
  assert.match(await page.locator('[data-slot="web-search"]').innerText(), /prefers-color-scheme theme initialization/);
  await shot("06-web-query");
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
    `Saved ${metrics.length} captures; copy, keyboard disclosures, scope, and overflow checks passed.`,
  );
} finally {
  await browser.close();
}
