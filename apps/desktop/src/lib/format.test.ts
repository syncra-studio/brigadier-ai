import assert from "node:assert/strict";
import { test } from "node:test";

import { formatDaySeparator, formatSentAt } from "@/lib/format";

test("a message from yesterday says Yesterday under it, as its day's separator does", () => {
  // Sunday 10:00 local; the message was sent Saturday 2:37 local.
  const now = new Date(2026, 9, 4, 10, 0).getTime();
  const sent = new Date(2026, 9, 3, 2, 37).getTime();
  const label = formatSentAt(sent, now);
  assert.match(label, /^Yesterday /);
  assert.equal(label, formatDaySeparator(sent, now));
});

test("today's messages show the time alone, older ones the weekday", () => {
  const now = new Date(2026, 9, 4, 10, 0).getTime();
  assert.doesNotMatch(formatSentAt(new Date(2026, 9, 4, 8, 0).getTime(), now), /day|Yesterday/);
  assert.doesNotMatch(formatSentAt(new Date(2026, 9, 1, 8, 0).getTime(), now), /Yesterday/);
});
