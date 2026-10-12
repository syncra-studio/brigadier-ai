import assert from "node:assert/strict";
import { test } from "node:test";

import type { ArtifactRef, Task } from "@/ipc/generated";

import { earlierShots, mockupOf, revisedSince, sentShots, shotNumber, shotsOf, takesNotes } from "./mockup";

const ref = (name: string, mime = "image/png"): ArtifactRef => ({
  id: `hash-${name}`, title: name, kind: "screenshot", mime, bytes: 1, fileName: name,
});

const task = (number: number, change: Partial<Task>): Task => ({
  id: `t${number}`, conversationId: "c1", number, title: `Task ${number}`, kind: "implement",
  state: "reported", outputs: [], reworkRounds: 0, ...change,
}) as Task;

test("a mockup's screenshots come in their numbers' order, other images after them", () => {
  assert.equal(shotNumber("mockup-12.png", "mockup"), 12);
  assert.equal(shotNumber("live-2.PNG", "live"), 2);
  assert.equal(shotNumber("mockup-1.txt", "mockup"), null);
  assert.equal(shotNumber("old-mockup-1.png", "mockup"), null);
  const outputs = [ref("mockup-10.png"), ref("notes.md", "text/markdown"), ref("zeta.png"), ref("mockup-2.png"), ref("alpha.png")];
  assert.deepEqual(shotsOf(outputs, "mockup").map((r) => r.fileName), ["mockup-2.png", "mockup-10.png"]);
  assert.deepEqual(shotsOf(outputs, "mockup", true).map((r) => r.fileName),
    ["mockup-2.png", "mockup-10.png", "alpha.png", "zeta.png"]);
});

test("the newest mockup pairs each screen with its live capture and side-by-side", () => {
  assert.equal(mockupOf({ t1: task(1, {}) }), null);
  const older = task(1, { mockup: { type: "builds" }, outputs: [ref("mockup-1.png")] });
  const built = task(3, { mockup: { type: "builds" }, state: "running", reworkRounds: 1,
    outputs: [ref("mockup-1.png"), ref("mockup-2.png")] });
  let mockup = mockupOf({ t1: older, t3: built })!;
  assert.equal(mockup.task.id, "t3");
  assert.equal(mockup.version, 2);
  assert.equal(mockup.status, "building");
  assert.equal(mockup.matching, null);
  assert.deepEqual(mockup.pairs.map((p) => [p.number, p.mockup?.fileName, p.live]), [[1, "mockup-1.png", null], [2, "mockup-2.png", null]]);

  mockup = mockupOf({ t3: { ...built, state: "reported" } })!;
  assert.equal(mockup.status, "ready");
  const matching = task(4, { mockup: { type: "matches", task: "t3" }, state: "running" });
  mockup = mockupOf({ t3: { ...built, state: "landed" }, t4: matching })!;
  assert.equal(mockup.status, "matching");
  mockup = mockupOf({ t3: built, t4: { ...matching, state: "reported", outputs: [ref("compare-2.png"), ref("live-2.png"), ref("live-1.png")] } })!;
  assert.equal(mockup.status, "compared");
  assert.deepEqual(mockup.pairs.map((p) => [p.mockup?.fileName, p.live?.fileName, p.compare?.fileName ?? null]),
    [["mockup-1.png", "live-1.png", null], ["mockup-2.png", "live-2.png", "compare-2.png"]]);
  // Real work sent back builds again: busy, not compared, though its captures stay.
  mockup = mockupOf({ t3: built, t4: { ...matching, state: "running", reworkRounds: 1, outputs: [ref("live-1.png")] } })!;
  assert.equal(mockup.status, "matching");
  // Screens go by their numbers, with gaps, and the mockup's unnumbered images after them.
  const sparse = task(6, { mockup: { type: "builds" }, outputs: [ref("mockup-10.png"), ref("extra.png"), ref("mockup-2.png")] });
  const real = task(7, { mockup: { type: "matches", task: "t6" }, outputs: [ref("live-10.png"), ref("live-2.png")] });
  assert.deepEqual(mockupOf({ t6: sparse, t7: real })!.pairs.map((p) => [p.number, p.mockup?.fileName, p.live?.fileName ?? null]),
    [[2, "mockup-2.png", "live-2.png"], [10, "mockup-10.png", "live-10.png"], [11, "extra.png", null]]);
  // A mockup stopped before any screenshot.
  assert.equal(mockupOf({ t5: task(5, { mockup: { type: "builds" }, state: "stopped" }) })!.status, "stopped");
});

test("notes' revision is there once the worker is done with new screenshots, or done after being sent back", () => {
  const shots = [ref("mockup-1.png")];
  const sent = { version: 1, shots: ["hash-mockup-1.png"] };
  const at = (change: Partial<Task>) => mockupOf({ t1: task(1, { mockup: { type: "builds" }, outputs: shots, ...change }) })!;
  assert.equal(revisedSince(at({}), sent), false, "not sent back yet");
  assert.equal(revisedSince(at({ state: "running", reworkRounds: 1 }), sent), false, "still working");
  assert.equal(revisedSince(at({ reworkRounds: 1 }), sent), true, "done with the same screenshots");
  assert.equal(revisedSince(at({ outputs: [ref("mockup-1b.png")] }), sent), true, "new screenshots");
});

test("the screenshots notes went on keep their screens when the revision drops one", () => {
  const before = mockupOf({ t1: task(1, { mockup: { type: "builds" }, outputs: [ref("mockup-1.png"), ref("mockup-2.png")] }) })!;
  const sent = sentShots(before);
  assert.deepEqual(sent.screens, [1, 2]);
  const after = mockupOf({ t1: task(1, { mockup: { type: "builds" }, outputs: [{ ...ref("mockup-2.png"), id: "hash-mockup-2-v2" }], reworkRounds: 1 }) })!;
  assert.deepEqual(earlierShots(after, sent).map((shot) => [shot.screen, shot.ref.id]), [[1, "hash-mockup-1.png"], [2, "hash-mockup-2.png"]]);
});

const notesTakenAt = (state: Task["state"]) =>
  takesNotes(mockupOf({ t1: task(1, { mockup: { type: "builds" }, outputs: [ref("mockup-1.png")], state }) })!);

test("notes are taken while the mockup's worker can get them, not once it has ended", () => {
  assert.equal(notesTakenAt("running"), true, "building");
  assert.equal(notesTakenAt("reported"), true, "ready");
  for (const state of ["stopped", "rejected", "failed", "landed", "done"] as const) {
    assert.equal(notesTakenAt(state), false, `${state}: its screenshots stay, without Comment`);
  }
});
