import assert from "node:assert/strict";
import { test } from "node:test";

import type { ArtifactRef, Task } from "@/ipc/generated";

import { mockupOf, shotNumber, shotsOf } from "./mockup";

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
  // A mockup stopped before any screenshot.
  assert.equal(mockupOf({ t5: task(5, { mockup: { type: "builds" }, state: "stopped" }) })!.status, "stopped");
});
