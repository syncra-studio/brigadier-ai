import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

type Window = { level: string; title: string; line: string[]; bar: string };
type Card = { name: string; inUse: boolean; checked: string; windows: Window[] };

// The real page in Chromium over a faked daemon with two Claude accounts and one Codex account:
// a card per account with what is left of each window, Activity's charts and their hover card,
// and a Refresh that spins until every account was checked for it.
test("Usage shows a card per account, their activity, and a Refresh that waits for every account", { timeout: 60000 }, async (t) => {
  const dump = await renderFixturePage(t, "usage.html?drive=1", "usage-result", 12000);
  const seen = JSON.parse(dump.replaceAll("&gt;", ">").replaceAll("&lt;", "<").replaceAll("&amp;", "&"));

  assert.deepEqual(seen.sections, ["Claude", "Codex"]);
  // Switching accounts is on: Claude's balancing shows once, over its cards.
  assert.equal(seen.balancing.length, 1);
  const cards = seen.cards as Card[];
  assert.equal(cards.length, 3, "one card per signed-in account");
  // Emails are masked as on Providers.
  assert.deepEqual(
    cards.map((card) => card.name),
    ["me•••••@•••••.com", "wor•••••@•••••.io", "me•••••@•••••.com"],
  );
  assert.deepEqual(
    cards.map((card) => card.inUse),
    [true, false, false],
    "In use only on the account work runs on",
  );
  // "Checked" comes from each account's own read time.
  assert.deepEqual(
    cards.map((card) => card.checked),
    ["Checked 4m ago", "Checked 12m ago", "Checked 2m ago"],
  );
  assert.deepEqual(cards[0]!.windows, [
    { level: "normal", title: "5-hour", line: ["Resets in 3h 48m", "91% left"], bar: "91%" },
    { level: "warning", title: "Weekly", line: ["Resets in 2d 5h", "18% left"], bar: "18%" },
    { level: "danger", title: "Weekly · Fable", line: ["Resets in 2d 5h", "7% left"], bar: "7%" },
  ]);
  assert.deepEqual(cards[1]!.windows.map((window) => window.line[1]), ["60% left", "45% left"]);
  assert.equal(seen.rowControls, 0, "the rows are static");

  assert.equal(seen.activity.charts, 3, "a chart per account");
  assert.equal(seen.activity.range, "7 days");
  assert.equal(seen.activity.forecasts.length, 2);
  for (const line of seen.activity.forecasts) assert.match(line, /Runs out ≈.*before it resets/);
  assert.match(seen.activity.tokens[0], /^Brigadier used 1\.6M tokens in the last 7 days$/);
  // Hovering a chart shows a card: the moment, then each window's share used then.
  assert.equal(seen.tooltip.length, 4);
  assert.deepEqual(
    seen.tooltip.slice(1).map((line: string) => line.replace(/\d+% used$/, "")),
    ["5-hour", "Weekly", "Weekly · Fable"],
  );
  assert.deepEqual(seen.range30, { range: "30 days", tokens: "Brigadier used 6.2M tokens in the last 30 days" });

  assert.deepEqual(seen.pressed, { spinning: "true", disabled: true });
  // A check that began before Refresh doesn't end it.
  assert.deepEqual(seen.stale, { spinning: "true", checked: seen.beforeRefresh });
  assert.deepEqual(seen.fresh, {
    spinning: "false",
    checked: ["Checked now", "Checked now", "Checked now"],
    disabled: false,
  });
  assert.deepEqual(seen.calls, ["activity 7", "activity 30", "refreshAccounts"]);
});
