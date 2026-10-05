// Production worker surfaces and browser behavior. Requires an installed Chromium/Playwright.
// PLAYWRIGHT_MODULE=<module> CHROME_BIN=<binary> node scripts/capture-workers-fixture.mjs <url> <shots> <reference-shots>
import assert from "node:assert/strict";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
const { chromium } = await import(process.env.PLAYWRIGHT_MODULE || "playwright");
const base = process.argv[2] || "http://127.0.0.1:15438";
const output = resolve(process.argv[3] || "worker-shots");
const references = process.argv[4];
await mkdir(output, { recursive: true });
const browser = await chromium.launch({ executablePath: process.env.CHROME_BIN, headless: true });
const page = await browser.newPage({ viewport: { width: 1281, height: 831 }, deviceScaleFactor: 2, reducedMotion: "reduce" });
const errors = [];
page.on("pageerror", (error) => errors.push(error.message));
const frozen = 1791192000000;
await page.addInitScript((now) => { Date.now = () => now; localStorage.setItem("brigadier.paneSizes", JSON.stringify({ workers: 600 })); }, frozen);
const measured = [];
async function shot(name, reference) {
  if (references && reference) {
    const png = await readFile(`${references}/${reference}`);
    await page.setViewportSize({ width: png.readUInt32BE(16) / 2, height: png.readUInt32BE(20) / 2 });
  }
  await page.mouse.move(2, 2);
  await page.screenshot({ path: `${output}/${name}.png`, animations: "disabled" });
  const stats = await page.locator('[data-slot="task-row"], [data-slot="workers-summary"], [data-slot="worker-thread"], [data-slot="worker-details"], [data-pane="workers"]').evaluateAll((nodes) => nodes.map((node) => {
    const rect = node.getBoundingClientRect(); const css = getComputedStyle(node);
    return { slot: node.dataset.slot ?? node.dataset.pane, x: rect.x, y: rect.y, width: rect.width, height: rect.height, fontSize: css.fontSize, lineHeight: css.lineHeight, gap: css.gap };
  }));
  measured.push({ name, reference: reference ?? null, stats });
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false);
}
const load = async (view, extra = "") => {
  await page.goto(`${base}/fixtures/workers-ux.html?summary=1&approvals=0&view=${view}${extra}`);
  await page.locator('[data-slot="request-work-header"]').waitFor();
  await page.locator('.aui-md').first().waitFor();
  if (!await page.getByRole("button", { name: "Open workers", exact: true }).isVisible())
    await page.getByRole("button", { name: /Toggle pinned summary|Toggle summary/ }).click();
  await page.getByRole("button", { name: "Open workers", exact: true }).waitFor();
};
const openList = async () => {
  const summary = page.getByRole("button", { name: "Open workers", exact: true });
  if (!await summary.isVisible()) await page.getByRole("button", { name: /Toggle pinned summary|Toggle summary/ }).click();
  await summary.click();
  await page.getByRole("heading", { name: /Active ·/ }).waitFor();
};
const openWorker = async (name) => {
  await page.getByRole("button", { name, exact: true }).click();
  await page.locator('[data-slot="worker-thread"]').waitFor();
};
try {
  await load("running");
  assert.equal(await page.locator('[data-slot="task-row"]').count(), 1);
  assert.match(await page.locator('[data-slot="task-row"]').innerText(), /File summary and Readme draft started working/);
  assert.equal(await page.locator('[data-slot="task-row"] button').count(), 2);
  assert.equal(await page.locator('[data-slot="background-workers"]').count(), 0);
  assert.match(await page.locator('[data-slot="workers-summary"]').innerText(), /2 working/);
  await shot("01-thread-running", "a-run-03.png");
  await shot("02-context-running", "c-summary-running.png");
  // Main-thread worker names are real entry points, including keyboard activation.
  await page.getByRole("button", { name: "Open File summary", exact: true }).focus();
  await page.keyboard.press("Enter");
  await page.locator('[data-slot="worker-thread"]').waitFor();
  assert.equal(await page.locator('[data-slot="worker-thread"] textarea, [data-slot="worker-thread"] [contenteditable]').count(), 0);
  assert.match(await page.locator('[data-slot="worker-thread"]').innerText(), /Listing files in . folder/);
  assert.doesNotMatch(await page.locator('[data-slot="worker-thread"]').innerText(), /Why this model|Workspace|Verification/);
  await shot("03-detail-running", "e-detail-running.png");
  await page.getByRole("button", { name: "Back to workers", exact: true }).click();
  assert.equal(await page.getByRole("button", { name: /is working$/ }).count(), 2);
  const rows = page.getByRole("button", { name: /is working$/ });
  const rowSizes = await rows.evaluateAll((nodes) => nodes.map((node) => ({ height: node.getBoundingClientRect().height, avatar: node.querySelector("svg")?.getBoundingClientRect().width })));
  assert.ok(rowSizes.every((row) => row.height === 60 && row.avatar === 24));
  measured.push({ name: "list-row-sizes", rowSizes });
  await shot("04-panel-running", "d-panel-overview-running.png");
  // The composer stop issues cancellation for the parent and both workers.
  await page.getByRole("button", { name: "Stop", exact: true }).click();
  await page.waitForFunction(() => window.flow.calls.filter((call) => call.method === "stopTask").length === 2);
  const calls = await page.evaluate(() => window.flow.calls);
  assert.equal(calls.filter((call) => call.method === "interrupt").length, 1);
  assert.deepEqual(calls.filter((call) => call.method === "stopTask").map((call) => call.taskId).toSorted(), ["t1", "t2"]);
  // Typing @ continues to list short job names after the strip is removed.
  const input = page.getByRole("textbox", { name: "Message input" });
  await input.fill("@");
  await page.getByRole("option", { name: /File summary/ }).waitFor();
  assert.doesNotMatch(await page.getByRole("listbox").innerText(), /task-\d+/);
  await input.fill("");

  await load("done");
  assert.match(await page.locator('[data-slot="workers-summary"]').innerText(), /2 done/);
  assert.equal(await page.locator('[data-slot="task-row"]').count(), 0, "Turn activity folds after completion");
  await shot("05-thread-done", "a-after-finish.png");
  await shot("06-context-done", "c-summary-done.png");
  await page.locator('[data-slot="request-work-header"]').click();
  assert.equal(await page.locator('[data-slot="task-row"]').count(), 3);
  assert.deepEqual(await page.locator('[data-slot="task-row"]').allInnerTexts(), ["File summary and Readme draft started working", "Readme draft finished", "File summary finished"]);
  assert.doesNotMatch(await page.locator('[data-slot="request-fold"]').innerText(), /Created|Used a tool|task-\d+|ls -la/);
  await shot("07-thread-expanded", "f-parent-worked-expanded.png");
  await openList();
  assert.match(await page.locator('[data-slot="side-panel-clip"]').innerText(), /Active · 0[\s\S]*No active workers[\s\S]*Done · 2/);
  await shot("08-panel-done", "d-panel-back-overview.png");
  await openWorker("File summary finished");
  const detail = page.locator('[data-slot="worker-thread"]');
  assert.match(await detail.innerText(), /6 previous messages/);
  assert.match(await detail.innerText(), /Sent message to parent/);
  assert.match(await detail.innerText(), /summary.md/);
  assert.match(await detail.innerText(), /Choose a license/);
  assert.doesNotMatch(await detail.innerText(), /Decisions|Why this model|Done when|Risks|Inspect every project file/);
  await shot("09-detail-done", "e-detail-done-top.png");
  await detail.getByRole("button", { name: "6 previous messages", exact: true }).click();
  assert.match(await detail.innerText(), /Inspect every project file/);
  await shot("10-detail-previous", "e-detail-expanded.png");
  await detail.getByRole("button", { name: "6 previous messages", exact: true }).click();
  await page.getByRole("button", { name: "Worker actions", exact: true }).click();
  await page.getByRole("menuitem", { name: "Details", exact: true }).click();
  const more = page.locator('[data-slot="worker-details"]');
  assert.match(await more.innerText(), /Decisions/);
  assert.match(await more.innerText(), /Verification/);
  assert.match(await more.innerText(), /Done when/);
  assert.match(await more.innerText(), /Instructions/);
  assert.match(await more.innerText(), /Why this model/);
  await shot("11-details", null);
  await page.getByRole("button", { name: "Back to worker", exact: true }).click();
  await page.getByRole("button", { name: "Back to workers", exact: true }).click();
  // Clear live transcript/process-only state as after restart/hibernate. Stored tasks/events stay.
  await page.evaluate(() => window.flow.useBoard.setState(({ board }) => ({ board: { ...JSON.parse(JSON.stringify(board)), transcripts: {}, activity: {}, summaries: {}, run: "idle" } })));
  await openWorker("File summary finished");
  await detail.getByText("Ran commands", { exact: false }).waitFor();
  assert.match(await detail.innerText(), /43 files/);
  assert.equal(await page.locator('[data-slot="worker-thread"] [contenteditable], [data-slot="worker-thread"] textarea').count(), 0);
  measured.push({ name: "restart-and-hibernate-reopen", passed: true });

  await load("running", "&waiting=1");
  await openList();
  const waiting = page.getByRole("button", { name: "Readme draft is waiting", exact: true });
  assert.match(await waiting.innerText(), /Waiting for its plan to be reviewed/);
  assert.equal(await waiting.locator('.shimmer').count(), 0);
  await shot("12-panel-waiting", "d-panel-one-done.png");
  await waiting.click();
  await page.locator('[data-slot="worker-thread"]').waitFor();
  await page.evaluate(() => window.flow.useBoard.setState(({ board }) => {
    const transcript = board.transcripts.t2;
    return { board: { ...board, transcripts: { ...board.transcripts, t2: { ...transcript,
      entries: [...transcript.entries, { streamSeq: 99, atMs: Date.now(), event: {
        type: "message", itemId: "waiting-reply", role: "assistant", text: "I’m waiting for the plan review before continuing.",
      } }],
    } } } };
  }));
  await detail.getByText("I’m waiting for the plan review before continuing.", { exact: true }).waitFor();
  assert.equal(await detail.locator('.group\\/answer').count(), 0, "A waiting reply is not a final answer");
  assert.equal(await detail.locator('.shimmer').count(), 0);
  measured.push({ name: "waiting-reply-stays-in-progress", passed: true });
  assert.deepEqual(errors, []);
  await writeFile(`${output}/measurements.json`, JSON.stringify({ measured, errors, calls }, null, 2));
  console.log(`Worker navigation, folding, stop, mentions, history and ${measured.length} captures/checks passed.`);
} finally { await browser.close(); }
