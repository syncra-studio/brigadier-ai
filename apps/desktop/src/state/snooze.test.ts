import assert from "node:assert/strict";
import { test } from "node:test";

import { snoozeTimes } from "./actions";

const local = (day: number, hour: number, minute = 0) => new Date(2026, 10, day, hour, minute).getTime();

test("snooze times: an hour on, this evening while it's later, tomorrow and next Monday at 9", () => {
  // Tuesday 3 November 2026, 14:30.
  assert.deepEqual(snoozeTimes(new Date(local(3, 14, 30))), [
    { label: "In 1 hour", untilMs: local(3, 15, 30) },
    { label: "This evening", untilMs: local(3, 18) },
    { label: "Tomorrow", untilMs: local(4, 9) },
    { label: "Next week", untilMs: local(9, 9) },
  ]);
  // From 17:00 an hour on is as late as the evening; on a Monday next week is a week on.
  assert.deepEqual(snoozeTimes(new Date(local(2, 17))).map((time) => time.label), ["In 1 hour", "Tomorrow", "Next week"]);
  assert.equal(snoozeTimes(new Date(local(2, 17))).at(-1)!.untilMs, local(9, 9));
  // On a Sunday next week is the next day.
  assert.equal(snoozeTimes(new Date(local(8, 10))).at(-1)!.untilMs, local(9, 9));
});
