import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

type Rendering = {
  first: { select: string; defaultRow: string; notice: string[]; picker: string };
  saved: { select: string; defaultRow: string; newSession: string; pickedForOne: string; session: string; picker: string };
  confirming: boolean;
  switched: { session: string; picker: string; notice: string[]; line: string };
  selection: { type: string; page: string };
  calls: string[];
};

// The real page and notice in Chromium over a faked daemon: Configuration saves the default a
// new session starts at and leaves the started session alone; the notice's button switches the
// session through the composer's confirmation and path, then gives way; its link opens the page.
test("Configuration saves the default, and the sandbox notice switches the session and opens Configuration", { timeout: 60000 }, async (t) => {
  // The page's dump is HTML: its ">" comes back escaped.
  const dump = await renderFixturePage(t, "access.html?drive=1", "access-result", 8000);
  const rendering = JSON.parse(dump.replaceAll("&gt;", ">").replaceAll("&lt;", "<").replaceAll("&amp;", "&")) as Rendering;
  assert.deepEqual(rendering.first, {
    select: "Ask for approval",
    defaultRow: "Ask for approval",
    notice: ["Switch this session to Full access", "Open Settings > Configuration"],
    picker: "Ask for approval",
  });
  assert.deepEqual(rendering.saved, {
    select: "Approve for me",
    defaultRow: "Approve for me",
    newSession: "approveForMe",
    pickedForOne: "fullAccess",
    session: "askForApproval",
    picker: "Ask for approval",
  });
  assert.equal(rendering.confirming, true);
  assert.deepEqual(rendering.switched, {
    session: "fullAccess",
    picker: "Full access",
    notice: ["Open Settings > Configuration"],
    line: "This session has Full access.",
  });
  assert.deepEqual(rendering.selection, { type: "settings", page: "configuration" });
  assert.deepEqual(rendering.calls, ["default approveForMe", "session fullAccess"]);
});
