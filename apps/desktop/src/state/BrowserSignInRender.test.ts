import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

// An agent's Browser tab at a page only the user goes through, in Chromium: the strip says why the agent stopped and
// what to do, Continue hands the tab back, Google's refusal has its own card, and a refused passkey its banner.
test("browser tool: a sign-in, a window, a captcha and Google's refusal each say why the agent stopped; the refusal's card and the passkey banner show", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "browser-sign-in.html?drive=1", "browser-sign-in-result", 60000)) as Record<string, unknown>;
  assert.equal(seen.before, false, "no strip while the agent works");

  const strip = (words: string) => ({ words, role: "status", continues: true });
  const stops = seen.stops as Record<string, unknown>;
  assert.deepEqual(stops.signIn, strip("This page asks you to sign in, so Check the shop stopped. Agents don't sign in: do it here, then press Continue."));
  assert.deepEqual(stops.window, strip("A sign-in window opened, so Check the shop stopped. Finish in that window, then press Continue."));
  assert.deepEqual(stops.captcha, strip("This page shows a captcha, so Check the shop stopped. Solve it, then press Continue."));
  assert.deepEqual(stops.rejected, strip("Google refused this sign-in, so Check the shop stopped. Press Continue to hand the page back."));
  for (const name of ["signIn", "window", "captcha", "rejected"]) assert.equal(stops[`${name}Continued`], true, `Continue ends the ${name} stop`);
  const tab = "browser:agent-1";
  assert.deepEqual(seen.told, Array.from({ length: 4 }, () => [{ tab, paused: true }, { tab, paused: false }]).flat());

  const card = seen.card as { words: string; role: string; height: number };
  assert.equal(card.words, "Google refused this sign-in. It decides which browsers it allows and has decided against this one. Your account and password are fine, but trying again here will probably get the same answer. Use your usual browser for this site if you need it now.");
  assert.equal(card.role, "status");
  assert.ok(card.height >= 32, "a card, not a line");
  assert.equal(seen.cardPaused, false, "the card alone stops no agent");
  assert.deepEqual(seen.opened, ["https://accounts.google.com/"], "Open in browser goes to Google in the user's own browser");
  assert.equal(seen.cardAfterOpen, true);
  assert.equal(seen.dismissed, true);
  assert.equal(seen.again, true);
  assert.equal(seen.pageLeft, true, "the page leaving the refusal takes the card with it");

  assert.equal(seen.passkey, "Passkeys don't work in Brigadier yet, so this page's request was refused. Sign in with a password or another option on the page.");
  assert.equal(seen.passkeyDismissed, true);
});
