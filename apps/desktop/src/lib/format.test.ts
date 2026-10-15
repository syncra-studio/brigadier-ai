import assert from "node:assert/strict";
import { test } from "node:test";

import { formatDaySeparator, formatSentAt, formatTime, separatesTurn } from "@/lib/format";

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

test("older messages: the short weekday and date with \"at\" within a year, the year after it", () => {
  const now = new Date(2026, 9, 15, 10, 0).getTime();
  const lastMonth = new Date(2026, 8, 18, 3, 9).getTime();
  assert.match(formatSentAt(lastMonth, now), /^\S+, Sep 18 at /);
  assert.ok(formatSentAt(lastMonth, now).endsWith(` at ${formatTime(lastMonth)}`));
  const lastYear = new Date(2025, 9, 3, 3, 9).getTime();
  assert.match(formatDaySeparator(lastYear, now), /^Oct 3, 2025 at /);
  assert.match(formatDaySeparator(new Date(2026, 9, 15, 8, 0).getTime(), now), /^Today /);
});

test("a time separator marks a user's message more than an hour after the one before, or an old first one", () => {
  const now = new Date(2026, 9, 15, 10, 0).getTime();
  const hour = 60 * 60 * 1000;
  const before = now - 3 * hour;
  // An hour exactly is no gap; a minute more is.
  assert.equal(separatesTurn(before + hour, before, now), false);
  assert.equal(separatesTurn(before + hour + 60_000, before, now), true);
  // The first message: only when sent more than an hour ago, across midnight or not.
  assert.equal(separatesTurn(now - 30 * 60_000, null, now), false);
  assert.equal(separatesTurn(now - 2 * hour, null, now), true);
});
