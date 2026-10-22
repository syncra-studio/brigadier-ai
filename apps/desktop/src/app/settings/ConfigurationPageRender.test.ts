import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive, renderFixturePage } from "@/fixtures/headless";

type Rendering = {
  first: { select: string; picker: string };
  saved: { select: string; newSession: string; pickedForOne: string; session: string; picker: string };
  confirming: boolean;
  switched: { session: string; picker: string };
  calls: string[];
};

// The real page and picker in Chromium over a faked daemon: Configuration saves the default a
// new session starts at and leaves the started session alone; the session's own picker switches
// it to Full access through the confirmation.
test("Configuration saves the default, and the session's picker switches it to Full access", { timeout: 60000 }, async (t) => {
  // The page's dump is HTML: its ">" comes back escaped.
  const dump = await renderFixturePage(t, "access.html?drive=1", "access-result", 8000);
  const rendering = JSON.parse(dump.replaceAll("&gt;", ">").replaceAll("&lt;", "<").replaceAll("&amp;", "&")) as Rendering;
  assert.deepEqual(rendering.first, { select: "Ask", picker: "Ask" });
  assert.deepEqual(rendering.saved, {
    select: "Accept edits",
    newSession: "acceptEdits",
    pickedForOne: "full",
    session: "ask",
    picker: "Ask",
  });
  assert.equal(rendering.confirming, true);
  assert.deepEqual(rendering.switched, { session: "full", picker: "Full access" });
  assert.deepEqual(rendering.calls, ["default acceptEdits", "session full"]);
});

test("a finished reply keeps its offer cards outside its collapsed work, and an old Full access suggestion isn't shown", { timeout: 60000 }, async (t) => {
  const dump = await renderFixturePage(t, "access-thread.html", "access-thread-result", 8000);
  const rendering = JSON.parse(dump.replaceAll("&gt;", ">").replaceAll("&lt;", "<").replaceAll("&amp;", "&"));
  assert.deepEqual(rendering, {
    state: "done",
    outsideWork: true,
    count: 0,
    // The session offer and the suggested setting both round to --radius-question; the setting
    // is the offer card: its icon in a tile, its buttons not indented.
    corners: [true, true],
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
  assert.deepEqual(seen.triggers, ["Automatic (Opus 5.5)", "Automatic (Opus 5.5)", "Accept edits"]);
  assert.deepEqual(seen.sessionsMenu, [["Automatic (Opus 5.5)", "The first signed-in agent’s default model."]]);
  assert.deepEqual(seen.chatsMenu, [["Automatic (Opus 5.5)", "The same model as new sessions."]]);
  assert.deepEqual(
    seen.permissionMenu.map((choice: string[]) => choice[0]),
    ["Ask", "Accept edits", "Full access"],
  );
  for (const [level, description] of seen.permissionMenu as string[][]) {
    assert.ok(description && description.length > 40, `${level} explains what it allows`);
  }
  assert.match(seen.permissionMenu[0][1], /safest/i);
  assert.match(seen.permissionMenu[1][1], /files can change/);
  assert.match(seen.permissionMenu[2][1], /delete or leak/);
  // The memories have a page of their own now (Memory, under Personal).
  assert.deepEqual(seen.oldLink, { page: { type: "settings", page: "memory" }, inView: true });
  assert.deepEqual(seen.search, { page: { type: "settings", page: "configuration" }, inView: true });
});
