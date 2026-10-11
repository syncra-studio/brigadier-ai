import assert from "node:assert/strict";
import { test } from "node:test";

import type { ImportProfile } from "@/ipc/generated";
import { byDay, dayLabel, firstProfile, importSummary, rangeStart, siteOf } from "@/state/browserData";

const at = (y: number, m: number, d: number, h = 12) => new Date(y, m - 1, d, h).getTime();

test("a page's site drops www. and keeps the port", () => {
  assert.equal(siteOf("https://www.example.com/a?b"), "example.com");
  assert.equal(siteOf("http://localhost:5173/"), "localhost:5173");
  assert.equal(siteOf("not a url"), "not a url");
});

test("history days: today, yesterday, a weekday this week, else the date", () => {
  const now = at(2026, 10, 11, 9);
  assert.equal(dayLabel(at(2026, 10, 11, 1), now), "Today");
  assert.equal(dayLabel(at(2026, 10, 10, 23), now), "Yesterday");
  const weekday = new Intl.DateTimeFormat(undefined, { weekday: "long" }).format(at(2026, 10, 7));
  assert.equal(dayLabel(at(2026, 10, 7), now), weekday);
  const date = new Intl.DateTimeFormat(undefined, { dateStyle: "medium" }).format(at(2026, 9, 1));
  assert.equal(dayLabel(at(2026, 9, 1), now), date);
});

test("visits newest first fall into one group per day", () => {
  const now = at(2026, 10, 11, 20);
  const visits = [at(2026, 10, 11, 18), at(2026, 10, 11, 8), at(2026, 10, 9, 22), at(2026, 10, 9, 1)].map(
    (atMs, id) => ({ id, atMs }),
  );
  const days = byDay(visits, now);
  assert.deepEqual(
    days.map((day) => [day.label, day.items.map((item) => item.id)]),
    [["Today", [0, 1]], [new Intl.DateTimeFormat(undefined, { weekday: "long" }).format(at(2026, 10, 9)), [2, 3]]],
  );
});

test("a cleared range starts that long ago; all time has no start", () => {
  const now = at(2026, 10, 11);
  assert.equal(rangeStart("hour", now), now - 3_600_000);
  assert.equal(rangeStart("month", now), now - 28 * 86_400_000);
  assert.equal(rangeStart("all", now), null);
});

const profile = (id: string, extra: Partial<ImportProfile> = {}): ImportProfile => ({
  id,
  browser: id,
  name: "Default",
  family: "gecko",
  hasCookies: true,
  hasHistory: true,
  readable: true,
  isDefault: false,
  running: false,
  ...extra,
});

test("an import starts on a default profile of a browser that isn't open", () => {
  assert.equal(
    firstProfile([profile("a"), profile("b", { isDefault: true, running: true }), profile("c", { isDefault: true })])?.id,
    "c",
  );
  assert.equal(firstProfile([profile("a", { running: true }), profile("b")])?.id, "b");
  assert.equal(firstProfile([profile("safari", { readable: false })])?.id, "safari");
  assert.equal(firstProfile([]), undefined);
});

test("an import's result says what came over and what didn't", () => {
  assert.deepEqual(
    importSummary("Firefox", {
      cookies: { imported: 1240, rejected: 2, skipped: 10, error: null },
      history: { imported: 1, error: null },
    }),
    [
      `Imported ${new Intl.NumberFormat().format(1240)} cookies and 1 page of history from Firefox.`,
      "12 cookies couldn't come across (expired, or a kind Brigadier's browser can't keep).",
    ],
  );
  assert.deepEqual(
    importSummary("Dia", {
      cookies: { imported: 0, rejected: 0, skipped: 0, error: "macOS didn't let Brigadier use Dia's cookie key, so no cookies were copied." },
      history: { imported: 3, error: null },
    }),
    ["Imported 3 pages of history from Dia.", "macOS didn't let Brigadier use Dia's cookie key, so no cookies were copied."],
  );
});
