import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

const DOTS = "••••••••";

// The real pages, prompt, star and delete dialog in Chromium over a faked shell, pressed with real clicks.
test("saved passwords, bookmarks, the save prompt, the star and the delete dialog", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "browser-passwords.html", "passwords-result", 80000)) as Record<string, unknown>;
  assert.equal(seen.failed, undefined);

  assert.ok((seen.pageSections as string[]).includes("Passwords and bookmarks"));

  // A list shows sites without www, and dots.
  assert.deepEqual(seen.sites, ["forum.example", "mail.example"]);
  assert.equal(seen.masked, DOTS);
  assert.equal(seen.neverSites, true);

  // The eye: cancelled, no Touch ID, passed, hidden again.
  assert.deepEqual(seen.declined, { message: true, value: DOTS });
  assert.equal(seen.unavailable, DOTS);
  assert.equal(seen.shown, "fake-Tulip-41");
  assert.equal(seen.hidden, DOTS);

  assert.equal(seen.removeAsks, true);
  assert.deepEqual(seen.passwordAsks, [["remove password", 2], ["allow", "https://bank.example"]]);

  assert.equal(seen.bookmarkRows, 2);
  assert.deepEqual(seen.folders, ["Bookmarks bar", "Bookmarks bar › Work"]);
  assert.deepEqual(seen.bookmarkAsks, [["remove bookmark", 2]]);

  // The prompt holds the site and username, no password, and its buttons wait a moment.
  assert.deepEqual(seen.offer, { says: "Save this password?fake-sam on site.exampleNever for this siteNot nowSave", armedAtOnce: false });
  assert.equal(seen.offerGone, true);
  assert.deepEqual(seen.update, { says: true, never: false });
  assert.equal(seen.otherTab, true);
  assert.deepEqual(seen.offerAsks, [["answer", "o1", "never"], ["answer", "o2", "save"]]);

  // The star: off, then on with its filled icon, then off. A page off the web has none.
  const off = seen.starOff as { pressed: string; name: string; icon: string };
  const on = seen.starOn as { pressed: string; name: string; icon: string };
  assert.equal(off.pressed, "false");
  assert.equal(off.name, "Bookmark this page");
  assert.equal(on.pressed, "true");
  assert.equal(on.name, "Remove bookmark");
  assert.notEqual(on.icon, off.icon, "pressed draws the filled star");
  assert.deepEqual(seen.starOffAgain, off);
  assert.equal(seen.starOnOtherPage, true);
  assert.deepEqual(seen.starAsks, [["add bookmark", "https://star.example/", "Star page", []], ["remove bookmark", 100]]);

  assert.equal(seen.deleteLine, "Its saved passwords are deleted with it. They don't move to Personal.");
});
