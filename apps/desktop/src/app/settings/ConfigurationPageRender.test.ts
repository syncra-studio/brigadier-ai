import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive, renderFixturePage } from "@/fixtures/headless";

type Rendering = {
  first: { select: string; notice: string[]; picker: string };
  saved: { select: string; newSession: string; pickedForOne: string; session: string; picker: string };
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
    notice: ["Switch this session to Full access", "Open Settings > Configuration"],
    picker: "Ask for approval",
  });
  assert.deepEqual(rendering.saved, {
    select: "Approve for me",
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

test("a finished reply keeps the sandbox notice outside its collapsed work, rounded like the offer cards", { timeout: 60000 }, async (t) => {
  const dump = await renderFixturePage(t, "access-thread.html", "access-thread-result", 8000);
  const rendering = JSON.parse(dump.replaceAll("&gt;", ">").replaceAll("&lt;", "<").replaceAll("&amp;", "&"));
  assert.deepEqual(rendering, {
    state: "done",
    outsideWork: true,
    buttons: ["Switch this session to Full access", "Open Settings > Configuration"],
    count: 1,
    // The notice, the session offer and the suggested setting all round to --radius-question;
    // the setting is the offer card: its icon in a tile, its buttons not indented.
    corners: [true, true, true],
    setting: { buttons: ["Turn on", "Open Settings"], tiled: true, indented: false },
  });
});

// The real page in Chromium over a faked daemon: one headless card of the defaults with a line
// each, every section shown, the explanations in the menus (Automatic first, each level's allowance and risk), and
// an old page's link and a search landing on their rows.
test("Configuration shows its defaults in one card and explains the choices in its menus", { timeout: 60000 }, async (t) => {
  const dump = await renderFixtureLive(t, "configuration.html?drive=1", "configuration-result", 30000);
  const seen = JSON.parse(dump.replaceAll("&gt;", ">").replaceAll("&lt;", "<").replaceAll("&amp;", "&"));
  assert.deepEqual(seen.firstCard, [
    { setting: "Model for sessions", helper: "Choose the model that leads new sessions, their orchestrator" },
    { setting: "Model for chats", helper: "Choose the model new chats start with" },
    { setting: "Default permission level", helper: "Choose how much new sessions may do without asking" },
    { setting: "Review the work", helper: "Choose whether another AI model reviews bigger or risky changes" },
    {
      setting: "Start workers on its own",
      helper: "Choose whether a session starts helpers when they help, or asks you first",
    },
  ]);
  assert.equal(seen.firstCardHeading, false, "the first card has no heading");
  // Every section shows.
  assert.deepEqual(seen.sections, ["Composer", "Brain"]);
  assert.deepEqual(seen.buttons, [], "no Use automatic button");
  assert.equal(seen.levelRow, false, "no read-only row for the chosen level");
  assert.deepEqual(seen.triggers, ["Automatic (Opus 5.5)", "Automatic (Opus 5.5)", "Approve for me"]);
  assert.deepEqual(seen.sessionsMenu, [["Automatic (Opus 5.5)", "The first signed-in agent’s default model."]]);
  assert.deepEqual(seen.chatsMenu, [["Automatic (Opus 5.5)", "The same model as new sessions."]]);
  assert.deepEqual(
    seen.permissionMenu.map((choice: string[]) => choice[0]),
    ["Ask for approval", "Approve for me", "Full access"],
  );
  for (const [level, description] of seen.permissionMenu as string[][]) {
    assert.ok(description && description.length > 40, `${level} explains what it allows`);
  }
  assert.match(seen.permissionMenu[0][1], /safest/i);
  assert.match(seen.permissionMenu[1][1], /reviewer can be wrong/);
  assert.match(seen.permissionMenu[2][1], /delete or leak/);
  // The memories have a page of their own now (Memory, under Personal).
  assert.deepEqual(seen.oldLink, { page: { type: "settings", page: "memory" }, inView: true });
  assert.deepEqual(seen.search, { page: { type: "settings", page: "configuration" }, inView: true });
});
