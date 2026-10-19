import assert from "node:assert/strict";
import { test } from "node:test";

import type { FoldSeen } from "@/fixtures/cardFold";
import { renderFixtureLive } from "@/fixtures/headless";

type Look = {
  state: string | null;
  title: string | null;
  standing: string | null;
  target: string | null;
  count: string | null;
  buttons: string[];
  heading: string | null;
  captions: string[];
  thumb: number | null;
  fit: string | null;
};
type Seen = {
  error?: string;
  cases: Record<"running" | "collapsed" | "paused" | "stopped" | "no-proofs" | "no-record" | "proofs-only", Look>;
  opened: string | null;
  browserTabs: number;
  menu: string[];
  unfolded: number;
  calls: string[];
};

const CAPTIONS = ["1. Added a todo with a due date 10:11 PM", "2. Checked off an overdue todo 10:12 PM"];

// The Preview card in each state of the user's design (K53xQ).
test("the Preview card: running with screenshots, folded, paused, stopped, no screenshots, no record, proofs alone", { timeout: 120000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "preview-card.html?drive=1", "preview-card-result", 60000)) as Seen;
  assert.equal(seen.error, undefined);
  const { cases } = seen;
  // Running: its state, address and Open; the screenshots inside, captioned, cropped to the top.
  const running = cases.running;
  assert.deepEqual([running.state, running.title, running.standing, running.target, running.count], ["running", "Web preview", "Running", "localhost:3000", null]);
  assert.deepEqual(running.buttons, ["Open", "Open in", "Fold the proofs"]);
  assert.equal(running.heading, "2 screenshots");
  assert.deepEqual(running.captions, CAPTIONS);
  assert.equal(running.thumb, 178);
  assert.equal(running.fit, "50% 0%");
  // Folded: the header counts the screenshots.
  assert.deepEqual([cases.collapsed.count, cases.collapsed.captions.length, cases.collapsed.buttons.at(-1)], ["2 screenshots", 0, "Show the proofs"]);
  assert.equal(seen.unfolded, 2);
  // Paused: Start again, the screenshots kept; Stopped: no Open to a dead page, the screenshots kept.
  assert.deepEqual([cases.paused.standing, cases.paused.buttons, cases.paused.captions], ["Paused", ["Start again", "Fold the proofs"], CAPTIONS]);
  assert.deepEqual([cases.stopped.state, cases.stopped.standing, cases.stopped.buttons, cases.stopped.captions], ["stopped", "Stopped", ["Fold the proofs"], CAPTIONS]);
  assert.ok(seen.calls.includes("resumePreview"));
  // No screenshots: the one line, no chevron.
  assert.deepEqual([cases["no-proofs"].buttons, cases["no-proofs"].heading], [["Open", "Open in"], null]);
  // No preview record: the address alone.
  assert.deepEqual([cases["no-record"].standing, cases["no-record"].target, cases["no-record"].state], [null, "localhost:5173", null]);
  // Proofs with no page left running: their card alone.
  const alone = cases["proofs-only"];
  assert.deepEqual([alone.title, alone.count, alone.buttons], ["Screenshots and recordings", "1 screenshot, 1 recording", ["Fold the proofs"]]);
  assert.deepEqual(alone.captions, ["1. The install section, fixed 10:11 PM", "2. install.webm 10:13 PM"]);
  // Open goes where local pages go (a Browser tab of the session); ▾ picks where.
  assert.equal(seen.opened, "http://localhost:3000/");
  assert.equal(seen.browserTabs, 1);
  assert.deepEqual(seen.menu, ["Brigadier browser", "Default browser"]);
});

// The whole header folds and opens the card, its buttons keep their own action, and the body's
// height and opacity animate, at once with reduced motion.
for (const reducedMotion of [false, true]) {
  test(`a click on the header folds and opens the card${reducedMotion ? ", at once with reduced motion" : ", animated"}`, { timeout: 120000 }, async (t) => {
    const seen = JSON.parse(await renderFixtureLive(t, "preview-card.html?case=running&drive=fold", "preview-card-result", 60000, { reducedMotion })) as FoldSeen & { error?: string };
    assert.equal(seen.error, undefined);
    // Open does its own thing; the card stays open.
    assert.equal(seen.afterButton, 2);
    assert.deepEqual(seen.folding, { state: "closed", animation: reducedMotion ? "none" : "collapsible-up" });
    assert.deepEqual(seen.unfolding, { state: "open", animation: reducedMotion ? "none" : "collapsible-down" });
    assert.equal(seen.reopened, 2);
    assert.notEqual(seen.closedWithinMs, null);
    if (reducedMotion) assert.ok(seen.closedWithinMs! < 100, `closed after ${seen.closedWithinMs} ms`);
    else assert.ok(seen.closedWithinMs! >= 150, `closed after ${seen.closedWithinMs} ms, not animated`);
  });
}
