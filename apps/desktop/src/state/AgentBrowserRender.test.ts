import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

// The browser tool's page script on a local test page in Chromium: the page read with a ref per
// control, clicks and typing as a page's own handlers hear them, and the actions that ask the
// user first (a declared send or payment, a password, a card, a one-time code) doing nothing
// until a yes given at the page's address.
test("browser tool: the page script reads, clicks, types and asks first", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "agent-browser.html", "agent-browser-result", 60000)) as Record<string, unknown>;
  if (process.env.AGENT_BROWSER_SEEN) console.log(JSON.stringify(seen, null, 2));
  const read = seen.read as { url: string; title: string; text: string };
  const page = read.url;
  assert.match(page, /\/fixtures\/agent-browser\.html$/);
  assert.equal(read.title, "Test shop");
  assert.deepEqual(read.text.split("\n"), [
    "# Test shop",
    "Welcome to the test shop. [image: A red mug]",
    `[e1] link "Docs" -> ${page}#docs`,
    "No link",
    "Count: 0",
    '[e2] button "Add one"',
    '[e3] button "Sold out" (disabled)',
    '[e4] textbox "Name"',
    '[e5] combobox "Size" value="Small" options=["Small","Large"]',
    '[e6] textbox "Note"',
    "## Chat",
    '[e7] textbox "Message"',
    "## Sign in",
    '[e8] textbox "Email"',
    '[e9] textbox "Password" (password field)',
    '[e10] button "Sign in"',
    '[e11] textbox "Card number" (card field)',
    '[e12] textbox "Code" (one-time code field)',
    '[e13] button "Pay $5"',
    '[e14] textbox "PIN" (password field)',
    '[e15] button "Unlock"',
    '[e16] checkbox "I agree" (not checked)',
  ], "hidden controls are left out; a label's text is its control's name");
  assert.equal(seen.again, true, "a second read keeps the refs");
  assert.match((seen.cut as { text: string }).text, /^# Test shop\nWelcome to the test shop\. \[i\n\(The page goes on: \d+ more characters not shown\.\)$/);

  const done = { done: true };
  assert.deepEqual([seen.click, seen.clicked], [done, "Count: 1"]);
  assert.deepEqual([seen.typeName, seen.name], [done, { value: "Ada", inputs: ["Ada"] }], "the page's input handler hears the text");
  assert.deepEqual([seen.typeSize, seen.size], [done, "l"], "a select takes an option by its text");
  assert.deepEqual([seen.typeNote, seen.note], [done, "Gift wrap"]);
  assert.deepEqual([seen.check, seen.checked], [done, true]);

  const ask = (sensitive: string, what: string) => ({ ask: { sensitive, what }, url: page });
  assert.deepEqual(seen.chat, done, "Enter sends a plain chat message");
  assert.deepEqual(seen.declaredSend, ask("send", 'textbox "Message"'), "Enter-to-send declared as sent in the user's name asks");
  assert.deepEqual(seen.sent, ["hello"], "nothing was sent while asking");
  assert.equal(seen.chatValue, "");

  assert.deepEqual(seen.email, done);
  assert.deepEqual(seen.password, ask("credentials", 'textbox "Password"'));
  assert.deepEqual([seen.passwordDeclared, seen.cardDeclared], [ask("credentials", 'textbox "Password"'), ask("card", 'textbox "Card number"')],
    "a declared sign-in or payment into a secret's field asks as the field, so its card never shows the secret");
  assert.deepEqual(seen.passwordElsewhere, ask("credentials", 'textbox "Password"'), "a yes given at another address doesn't hold");
  assert.equal(seen.passwordEmpty, "");
  assert.deepEqual([seen.passwordYes, seen.passwordValue], [done, "hunter2"]);
  assert.deepEqual(seen.signIn, ask("credentials", 'button "Sign in"'), "submitting a filled password asks");
  assert.deepEqual(seen.signInYes, done);
  assert.deepEqual(seen.card, ask("card", 'textbox "Card number"'));
  assert.deepEqual(seen.code, ask("oneTimeCode", 'textbox "Code"'));
  assert.deepEqual(seen.declaredPay, ask("pay", 'button "Pay $5"'));
  assert.deepEqual(seen.submitted, ["login"], "only the sign-in with its yes went");
  assert.deepEqual(seen.unlock, ask("credentials", 'button "Unlock"'), "a button beside a filled password asks, form or not");

  assert.deepEqual(seen.stale, { error: "There is no e999 on the page now: read it again for its controls' refs." });
  assert.deepEqual(seen.notField, { error: "e2 isn't a field to type in (it is a button)." });
  assert.deepEqual([seen.link, seen.hash], [done, "#docs"], "a link goes, a filled password elsewhere on the page or not");
  assert.deepEqual(seen.filled, [
    '[e9] textbox "Password" (filled) (password field)',
    '[e11] textbox "Card number" (filled) (card field)',
    '[e12] textbox "Code" (filled) (one-time code field)',
    '[e14] textbox "PIN" (filled) (password field)',
    '[e17] textbox "Shown password" (filled) (password field)',
  ], "no secret's value is read out");
});
