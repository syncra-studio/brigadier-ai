// Checks that a finished turn's work folds and unfolds with no flicker (THREAD-PARITY-PLAN.md §4.2):
// in headless Chromium, every painted frame while the work opens and closes, what follows the work
// moves by exactly as much as the work's height changed, in one direction, and the header stays
// where it was clicked. Run against Vite:
//   PLAYWRIGHT_MODULE=<module path> CHROME_BIN=<binary> node scripts/check-fold-motion.mjs <base-url>
import assert from "node:assert/strict";

const { chromium } = await import(process.env.PLAYWRIGHT_MODULE || "playwright");
const base = process.argv[2] || "http://127.0.0.1:1420";
/** A frame's movement may differ from the height's change by subpixel rounding only. */
const TOLERANCE_PX = 0.5;
/** The thread scrolls by whole pixels to keep the header in place: it may be off by one. */
const SCROLL_ROUNDING_PX = 1;

const CASES = [
  // The user's session of 2026-10-09: its first turn has a message steered into it.
  { name: "with a steer", page: "thread-session.html?session=grill&done=1", block: "steered" },
  // The same session's last turn, and T1's turn: no steers.
  { name: "without steers (grill)", page: "thread-session.html?session=grill&done=1", block: "last" },
  { name: "without steers (T1)", page: "thread-session.html?done=1", block: "last" },
];

const browser = await chromium.launch({ executablePath: process.env.CHROME_BIN, headless: true });
const failures = [];
try {
  for (const density of ["normal", "compact"]) {
    for (const check of CASES) {
      const page = await browser.newPage({ viewport: { width: 1728, height: 1024 }, deviceScaleFactor: 2, colorScheme: "dark" });
      const errors = [];
      page.on("pageerror", (error) => errors.push(error.message));
      await page.goto(`${base}/fixtures/${check.page}&density=${density}`);
      await page.locator('[data-slot="request-work-header"]').first().waitFor();
      await page.waitForTimeout(500);
      const label = `${check.name}, ${density}`;
      for (const action of ["open", "close"]) {
        const result = await page.evaluate(sampleToggle, { block: check.block, action });
        const problems = judge(result);
        console.log(`${label}, ${action}: ${result.frames.length} frames, work ${result.from.toFixed(1)} → ${result.to.toFixed(1)}px, below moved ${(result.moved - result.headerMoved).toFixed(1)}px, header moved ${result.headerMoved.toFixed(1)}px${problems.length ? ` — ${problems.join("; ")}` : ""}`);
        for (const problem of problems) failures.push(`${label}, ${action}: ${problem}`);
      }
      assert.deepEqual(errors, [], `${label}: page errors`);
      await page.close();
    }
  }
} finally {
  await browser.close();
}
if (failures.length > 0) {
  console.error(`\n${failures.length} problem(s):\n${failures.join("\n")}`);
  process.exit(1);
}
console.log("\nNo flicker: what follows the work moves with its height, frame by frame.");

/**
 * Runs in the page: toggles the chosen block's work and records, for every painted frame, the
 * work's height, the top of what follows it and the header's top.
 */
async function sampleToggle({ block, action }) {
  const headers = [...document.querySelectorAll('[data-slot="request-work-header"]')].filter((header) => header.tagName === "BUTTON");
  const header =
    block === "steered"
      ? headers.find((candidate) => candidate.closest('[data-slot="aui_assistant-message-root"]')?.querySelector('[data-slot="request-steer"]'))
      : headers.at(-1);
  if (!header) throw new Error(`no ${block} block`);
  const root = header.closest('[data-slot="aui_assistant-message-root"]');
  const scroller = header.closest('[data-slot="aui_thread-viewport"]');
  // The header a third of the way down the view, as a reader would click it.
  scroller.scrollTop += header.getBoundingClientRect().top - scroller.getBoundingClientRect().top - scroller.clientHeight / 3;
  await new Promise((resolve) => setTimeout(resolve, 300));
  if ((header.getAttribute("aria-expanded") === "true") !== (action === "close")) throw new Error(`the work isn't ${action === "close" ? "open" : "closed"}`);
  // What follows the work: the answer, else the block's actions.
  const below = root.querySelector('[data-slot="request-body"] > [data-slot="aui_assistant-message-content"]') ?? root.lastElementChild;
  const workHeight = () => [...root.querySelectorAll('[data-slot="request-fold"]')].reduce((sum, fold) => sum + fold.getBoundingClientRect().height, 0);
  const sample = () => ({
    time: document.timeline.currentTime,
    work: workHeight(),
    below: below.getBoundingClientRect().top,
    header: header.getBoundingClientRect().top,
  });
  const samples = [sample()];
  // The animation time of the last rendering update that resized the turn: its resize observers,
  // the anchor's scroll fix among them, ran before that frame was painted.
  let updated = null;
  const observer = new ResizeObserver(() => {
    updated = document.timeline.currentTime;
  });
  let sampling = true;
  // After each frame is painted, what it showed.
  const tick = () => {
    if (!sampling) return;
    requestAnimationFrame(() => setTimeout(() => {
      const frame = sample();
      samples.push({ ...frame, updated: updated === frame.time });
      tick();
    }, 0));
  };
  header.click();
  observer.observe(root);
  tick();
  await new Promise((resolve) => setTimeout(resolve, 700));
  sampling = false;
  observer.disconnect();
  // Outside a frame, Chromium's animation clock runs ahead to the next frame's time, so a timeout
  // that runs late measures a frame not yet laid out, observed or fixed: a resize no update has
  // seen, never painted. Such a sample is left out; the rest are what was painted.
  const frames = [samples[0]];
  for (const frame of samples.slice(1)) {
    if (!frame.updated && Math.abs(frame.work - frames.at(-1).work) > 0.01) continue;
    if (frame.time === frames.at(-1).time) frames.pop();
    frames.push(frame);
  }
  const last = frames.at(-1);
  return {
    frames,
    from: frames[0].work,
    to: last.work,
    moved: last.below - frames[0].below,
    headerMoved: last.header - frames[0].header,
  };
}

/** How far below the header what follows the work is. */
function gap(frame) {
  return frame.below - frame.header;
}

/**
 * What is wrong with a toggle's frames, if anything. What follows is measured from the header, so
 * the thread's own scroll (whole pixels, while layout is fractional) cancels out; the header
 * itself may drift by that rounding only.
 */
function judge({ frames, from, to }) {
  const problems = [];
  if (Math.abs(from - to) < 1) problems.push("the work didn't change height");
  const direction = Math.sign(to - from);
  for (let index = 1; index < frames.length; index++) {
    const before = frames[index - 1];
    const now = frames[index];
    const grew = now.work - before.work;
    const step = gap(now) - gap(before);
    if (Math.abs(step - grew) > TOLERANCE_PX) {
      problems.push(`frame ${index}: what follows moved ${step.toFixed(2)}px while the work changed ${grew.toFixed(2)}px`);
    }
    if (step * direction < -TOLERANCE_PX) problems.push(`frame ${index}: moved back ${step.toFixed(2)}px`);
    if (Math.abs(now.header - frames[0].header) > SCROLL_ROUNDING_PX) {
      problems.push(`frame ${index}: the header moved ${(now.header - frames[0].header).toFixed(2)}px`);
    }
  }
  const moved = gap(frames.at(-1)) - gap(frames[0]);
  if (Math.abs(moved - (to - from)) > TOLERANCE_PX) problems.push(`what follows ended ${moved.toFixed(2)}px away, the work changed ${(to - from).toFixed(2)}px`);
  return [...new Set(problems)].slice(0, 6);
}
