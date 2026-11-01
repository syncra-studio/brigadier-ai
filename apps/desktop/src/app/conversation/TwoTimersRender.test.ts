import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

const seconds = (label: string | null) => Number(/(\d+)s$/.exec(label ?? "")?.[1] ?? NaN);

test("a working block's header and the composer read the same time at every moment, from the message sent, not the CLI's later start (D94)", { timeout: 60000 }, async (t) => {
  const reads = JSON.parse(await renderFixturePage(t, "two-timers.html", "two-timers-result", 10000)) as [string | null, string | null][];
  for (const [header, composer] of reads) {
    assert.match(header ?? "", /^Working for 1[2-6]s$/, JSON.stringify(reads));
    assert.equal(composer, header, JSON.stringify(reads));
  }
});

test("a block its background subagent holds open after its turn reads the same time in its header and the composer, and keeps it once the subagent ends (D24, D94, D104)", { timeout: 60000 }, async (t) => {
  const { held, after } = JSON.parse(await renderFixturePage(t, "two-timers.html", "two-timers-held", 20000)) as { held: [string | null, string | null][]; after: [string | null, string | null] };
  for (const [header, composer] of held) {
    assert.match(header ?? "", /^Working for \d+s$/, JSON.stringify(held));
    assert.equal(composer, header, JSON.stringify(held));
  }
  // Over, it worked until the subagent ended: the time never drops back to the turn's own.
  assert.match(after[0] ?? "", /^Worked for \d+s$/, JSON.stringify(after));
  assert.ok(seconds(after[0]) >= seconds(held.at(-1)![0]), JSON.stringify({ held, after }));
  assert.doesNotMatch(after[1] ?? "", /Working for/, JSON.stringify(after));
});
