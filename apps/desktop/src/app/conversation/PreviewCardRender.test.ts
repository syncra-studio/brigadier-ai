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
  where: string | null;
  shell: [string | null, string | null] | null;
  captions: string[];
  thumb: number | null;
  fit: string | null;
};
type Seen = {
  error?: string;
  cases: Record<"running" | "collapsed" | "paused" | "stopped" | "no-proofs" | "no-record" | "proofs-only" | "test-output" | "simulator" | "pdf" | "test-run" | "test-failed" | "test-uncounted", Look>;
  turn: [string | null, string | null][];
  said: [string | null, string | null][][];
  noRecord: boolean;
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
  assert.deepEqual([running.where, running.heading], ["Checked in the browser", "2 screenshots"]);
  assert.deepEqual(running.captions, CAPTIONS);
  assert.equal(running.thumb, 178);
  assert.equal(running.fit, "50% 0%");
  // Folded: the header counts the screenshots.
  assert.deepEqual([cases.collapsed.count, cases.collapsed.captions.length, cases.collapsed.buttons.at(-1)], ["2 screenshots", 0, "Show the proofs"]);
  assert.equal(seen.unfolded, 2);
  // Paused: Start again, the screenshots kept; Stopped: no Open to a dead page but Start again, the screenshots kept.
  assert.deepEqual([cases.paused.standing, cases.paused.buttons, cases.paused.captions], ["Paused", ["Start again", "Fold the proofs"], CAPTIONS]);
  assert.deepEqual([cases.stopped.state, cases.stopped.standing, cases.stopped.buttons, cases.stopped.captions], ["stopped", "Stopped", ["Start again", "Fold the proofs"], CAPTIONS]);
  assert.ok(seen.calls.includes("resumePreview"));
  // Start again and Run again run an ended preview's own command; a ready PDF's Open opens its file,
  // a running simulator's brings Simulator forward on its device.
  assert.ok(seen.calls.includes("restartPreview preview-3"));
  assert.ok(seen.calls.includes("restartPreview preview-test"));
  assert.ok(seen.calls.includes("open_pdf /w/app/out/invoice.pdf"));
  assert.ok(seen.calls.includes("open_simulator iPhone 17"));
  // The other kinds: each its badge's title, its state, its target and what was taken of it.
  const sim = cases.simulator;
  assert.deepEqual([sim.title, sim.standing, sim.target, sim.count, sim.buttons], ["iOS Simulator", "Running", "iPhone 17", "1 recording", ["Open", "Fold the proofs"]]);
  assert.deepEqual(sim.captions, ["1. Adding a todo on the phone 10:11 PM"]);
  const pdf = cases.pdf;
  assert.deepEqual([pdf.title, pdf.standing, pdf.target, pdf.buttons], ["Rendered PDF", "Ready", "out/invoice.pdf", ["Open"]]);
  const run = cases["test-run"];
  assert.deepEqual([run.title, run.standing, run.target, run.buttons], ["Test run", "15 passed", "npm test", ["Run again", "Fold the proofs"]]);
  assert.deepEqual(run.shell, [
    "$ npm test",
    "✓ rejects a title longer than 200 characters\n✓ accepts years 0000 to 0099\n✓ marks a past due date as overdue\n✓ a todo due today is not overdue\n15 passing (412ms)",
  ]);
  // A test run's counts as its runner gave them; none found, Passed or Failed alone.
  assert.deepEqual([cases["test-failed"].standing, cases["test-failed"].state], ["2 failed, 13 passed, 1 skipped", "stopped"]);
  assert.deepEqual([cases["test-uncounted"].standing, cases["test-uncounted"].state], ["Passed", "running"]);
  // A turn's previews: the page and the test runs and PDF started while its request ran, the
  // simulator its recording names, not the run started after it.
  assert.deepEqual(seen.turn, [
    ["Web preview", "localhost:4000"],
    ["iOS Simulator", "iPhone 17"],
    ["Rendered PDF", "out/invoice.pdf"],
    ["Test run", "npm test"],
    ["Test run", "npm test"],
  ]);
  // No screenshots: the one line, no chevron.
  assert.deepEqual([cases["no-proofs"].buttons, cases["no-proofs"].heading], [["Open", "Open in"], null]);
  // An address with no preview record behind it is no card (#438): a turn that started nothing
  // has no page card for an address its answer names, nor for an older preview's that stopped; an
  // older preview still running on it is the page, route and all.
  assert.equal(seen.noRecord, false);
  assert.deepEqual(seen.said, [[], [], [["Web preview", "localhost:3000/#/todos"]]]);
  // Proofs with no page left running: their card alone.
  const alone = cases["proofs-only"];
  assert.deepEqual([alone.title, alone.count, alone.buttons], ["Screenshots and recordings", "1 screenshot, 1 recording", ["Fold the proofs"]]);
  assert.deepEqual(alone.captions, ["1. The install section, fixed 10:11 PM", "2. install.webm 10:13 PM"]);
  // A test run's output: its last five lines under its command, counted as a log.
  const tests = cases["test-output"];
  assert.deepEqual([tests.title, tests.count, tests.buttons], ["Test output", "1 log", ["Fold the proofs"]]);
  assert.deepEqual(tests.shell, [
    "$ npm test",
    "✓ rejects a title longer than 200 characters\n✓ accepts years 0000 to 0099\n✓ marks a past due date as overdue\n✓ a todo due today is not overdue\n15 passing (412ms)",
  ]);
  assert.deepEqual(tests.captions, ["1. The todo tests 10:15 PM"]);
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
