import assert from "node:assert/strict";
import { test } from "node:test";

import benchRun from "@/fixtures/boards/computer-bench-run.json" with { type: "json" };
import type { ComputerAction } from "@/ipc/generated";
import { replayComputer } from "@/replay/computer";

// One batch of each kind from a `brigadier-computer bench --quick --replay` run on the fixture app.
const actions = benchRun as ComputerAction[];

test("the timeline replays a bench run: a batch per act, each step's words and outcome", () => {
  const frames = replayComputer("bench", "bench-worker", actions);
  assert.equal(frames.length, actions.length);
  assert.deepEqual(frames.map((frame) => frame.batches), [1, 2, 3, 4, 5, 6, 7, 7, 8]);
  assert.deepEqual(frames.map((frame) => frame.shown.at(-1)), [
    { words: "Clicked “Button 8 pt”", outcome: "done", failed: false },
    { words: "Clicked “Check 8 pt”", outcome: "worked", failed: false },
    { words: "Set “Level”", outcome: "worked", failed: false },
    { words: "Chose Targets › Plain Item", outcome: "done", failed: false },
    { words: "Clicked a point", outcome: "done", failed: false },
    { words: "Clicked “Minimised Target”", outcome: "couldn't do it in the background", failed: true },
    { words: "Selected text in “Notes”", outcome: "worked", failed: false },
    { words: "Typed text", outcome: "worked", failed: false },
    { words: "Clicked “Minimised Target”", outcome: "done", failed: false },
  ]);
  // The select and the typing over it were one act: one batch, both steps, its image on it.
  assert.equal(frames[7]!.shown.length, 2);
  assert.equal(frames[7]!.image, "bench-00442");
  // A menu pick aims at no point, so it has no image.
  assert.equal(frames[3]!.image, null);
  assert.equal(frames.at(-1)!.summary, "Used the computer · 9 steps in target-range");
});

test("a replayed action read again (history and live overlap) is shown once", () => {
  const twice = [...actions, ...actions.slice(0, 3)];
  const frames = replayComputer("bench", "bench-worker", twice);
  assert.equal(frames.at(-1)!.batches, 8);
  assert.equal(frames.at(-1)!.summary, "Used the computer · 9 steps in target-range");
});
