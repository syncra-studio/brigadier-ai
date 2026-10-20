import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

type Row = { setting: string; helper: string; dimmed: boolean };
type Rendering = {
  sections: string[];
  rows: Row[];
  version: string;
  followUpHelper: string;
  followUp: string[];
  loginBefore: string;
  loginOn: string;
  loginOff: string;
  licenses: { title: string; mentions: boolean };
  notificationsAfter: string[];
  calls: string[];
};

const render = async (t: Parameters<typeof renderFixturePage>[0], query: string) => {
  // The page's dump is HTML: its ">" comes back escaped.
  const dump = await renderFixturePage(t, `general.html?drive=1&${query}`, "general-result", 9000);
  return JSON.parse(dump.replaceAll("&gt;", ">").replaceAll("&lt;", "<").replaceAll("&amp;", "&")) as Rendering;
};

// The real page in Chromium over a faked shell and daemon: five sections with a line under every
// row, each choice saved as the setting the composer and the notifications read, Open at login
// turned on and off through the system, the permission asked for, and the licenses shown.
test("General: its sections and rows, and every new setting saved for real", { timeout: 60000 }, async (t) => {
  const seen = await render(t, "notifications=notAsked");
  assert.deepEqual(seen.sections, ["Appearance", "Composer", "Notifications", "Power", "About"]);
  assert.deepEqual(
    seen.rows.map((row) => row.setting),
    [
      "Your name",
      "When the sidebar is closed",
      "Follow-up behaviour",
      "Plain text composer",
      "Show context window usage",
      "Notifications aren't on yet",
      "Notify when a session finishes",
      "When something needs you",
      "Keep the computer awake",
      "Keep going with the lid closed",
      "Hibernate after",
      "Open at login",
      "Version",
      "Open source licenses",
      "Run setup again",
    ],
  );
  // One short line under every row, without a closing period.
  for (const row of seen.rows) {
    assert.ok(row.helper.length > 0 && row.helper.length <= 100, `${row.setting}: ${row.helper}`);
    assert.ok(!row.helper.endsWith("."), `${row.setting} ends without a period`);
  }
  // Until notifications are on, the rows they govern are dimmed, not hidden.
  assert.deepEqual(
    seen.rows.filter((row) => row.dimmed).map((row) => row.setting),
    ["Notify when a session finishes", "When something needs you"],
  );
  assert.equal(seen.version, "Brigadier 0.9.0");
  // Enter always sends (no Send shortcut row); ⌥Enter sends the other way.
  assert.equal(seen.followUpHelper, "What a message sent while an agent works does; ⌥Enter does the other for one message");
  assert.deepEqual(seen.followUp, ["Steer"]);
  assert.equal(seen.loginOn, "true");
  assert.equal(seen.loginOff, "false");
  assert.deepEqual(seen.licenses, { title: "Open source licenses", mentions: true });
  // Allowed, the status row goes and the rows light up.
  assert.deepEqual(seen.notificationsAfter, ["Notify when a session finishes", "When something needs you"]);
  assert.deepEqual(seen.calls, [
    "followUp steer",
    "plainTextComposer true",
    "notifyNeedsYou false",
    "setLoginItem true",
    "setLoginItem false",
    "askNotifications",
  ]);
});

test("General: notifications off point to System Settings, and Open at login waits for approval", { timeout: 60000 }, async (t) => {
  const seen = await render(t, "notifications=off&login=needsApproval");
  const notifications = seen.rows.slice(5, 8);
  assert.deepEqual(notifications, [
    { setting: "Notifications are off", helper: "Turn them on in System Settings to use the options below", dimmed: false },
    { setting: "Notify when a session finishes", helper: "A notification when a chat or session is done", dimmed: true },
    { setting: "When something needs you", helper: "Approvals and questions waiting for your answer", dimmed: true },
  ]);
  assert.equal(
    seen.rows.find((row) => row.setting === "Open at login")?.helper,
    "Allow Brigadier in System Settings › Login Items to finish turning this on",
  );
  // Waiting for approval reads as on, so the switch can take the request back.
  assert.equal(seen.loginBefore, "true");
  assert.equal(seen.calls.find((call) => call.startsWith("setLoginItem")), "setLoginItem false");
  assert.ok(seen.calls.includes("openNotificationSettings"));
});
