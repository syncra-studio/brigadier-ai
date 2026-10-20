import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

/** The fixture's JSON as the page's DOM escaped it. */
const unescape = (dump: string) => dump.replaceAll("&gt;", ">").replaceAll("&lt;", "<").replaceAll("&amp;", "&");

const ACCEPTS = "Accept Current Change|Accept Incoming Change|Accept Both Changes|Compare Changes";

test("a conflicted file's tab accepts sides in place, compares them, and completes the merge in the merge editor", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(unescape(await renderFixturePage(t, "conflict-tab.html", "conflict-tab-result", 8000))) as Record<string, unknown>;
  assert.equal(seen.error, undefined, JSON.stringify(seen));
  assert.deepEqual(seen.lenses, [ACCEPTS, ACCEPTS]);
  // The current side is blue, the incoming one green; their marker lines stronger.
  assert.deepEqual((seen.tints as string[]).slice(0, 4), [
    "bg-link/30:<<<<<<< HEAD (Current Change)",
    "bg-link/15:  return cart.member ? 9 : 10;",
    "bg-success/15:  return couponFor(cart).apply(10);",
    "bg-success/30:>>>>>>> main (Incoming Change)",
  ]);
  assert.equal(seen.mergeButton, true);
  const first = seen.firstWrite as { path: string; expected: string; text: string; stage: boolean };
  assert.equal(first.path, "src/cart/total.ts");
  assert.equal(first.stage, false, "accepting a side doesn't stage the file");
  assert.match(first.expected, /^import/, "written only over the file as it was read");
  assert.match(String(seen.afterIncoming), /^import .*\nexport function total\(cart\) \{\n {2}return couponFor\(cart\)\.apply\(10\);\n\}\n/);
  assert.match(String(seen.afterBoth), /\n {2}return cart\.total > 50 \? 0 : 4\.99;\n {2}return cart\.express \? 9\.99 : 4\.99;\n\}\n$/);
  assert.equal(seen.mergeButtonGone, true, "no conflicts left, no merge editor");
  assert.equal(seen.compareTab, "compare:src/cart/total.ts");

  assert.deepEqual(seen.panes, [
    "Incoming11e363f • main",
    "Current0288795 • brigadier/912a9469/session",
    "Resultsrc/cart/total.ts2 Conflicts Remaining",
  ]);
  assert.deepEqual(seen.resultLenses, ["No Changes Accepted", "No Changes Accepted"]);
  assert.deepEqual(seen.afterAccept, ["Incoming|Remove Incoming", "No Changes Accepted"]);
  assert.deepEqual(seen.incomingLenses, ["", "Accept Incoming|Accept Combination (Incoming First)|Ignore"]);
  assert.deepEqual(seen.currentLenses, [
    "Accept Current|Accept Combination (Current First)",
    "Accept Current|Accept Combination (Current First)|Ignore",
  ]);
  assert.match(String(seen.asked), /Complete the merge of total\.ts\?1 conflict isn't handled yet/);
  const done = seen.completed as { text: string; stage: boolean };
  assert.equal(done.stage, true, "Complete Merge stages the file");
  assert.equal(
    done.text,
    "import { couponFor } from \"./coupon\";\nexport function total(cart) {\n  return couponFor(cart).apply(10);\n}\nexport function shipping(cart) {\n  return 4.99;\n}\n",
  );
});
