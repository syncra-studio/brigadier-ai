import assert from "node:assert/strict";
import { test } from "node:test";

import { LabelPace } from "@/app/conversation/activity/calmLabel";

test("a live label changes at most once a second, and the newest label wins when its turn comes", () => {
  const pace = new LabelPace(1000);
  // The first label shows at once.
  assert.deepEqual(pace.next("Reading a.ts", 0), { shown: "Reading a.ts", wakeAt: null });
  // Fast steps wait for the second to be up; only the newest is kept.
  assert.deepEqual(pace.next("Reading b.ts", 200), { shown: "Reading a.ts", wakeAt: 1000 });
  assert.deepEqual(pace.next("Reading c.ts", 600), { shown: "Reading a.ts", wakeAt: 1000 });
  assert.deepEqual(pace.next("Reading c.ts", 1000), { shown: "Reading c.ts", wakeAt: null });
  // The same label asks nothing.
  assert.deepEqual(pace.next("Reading c.ts", 1100), { shown: "Reading c.ts", wakeAt: null });
  // A label after a calm second shows at once.
  assert.deepEqual(pace.next("Thinking", 2500), { shown: "Thinking", wakeAt: null });
  // Back to an earlier label inside the second: the shown one stays, nothing waits.
  assert.deepEqual(pace.next("Running npm test", 2700), { shown: "Thinking", wakeAt: 3500 });
  assert.deepEqual(pace.next("Thinking", 2800), { shown: "Thinking", wakeAt: null });
});
