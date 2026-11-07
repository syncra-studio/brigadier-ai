import assert from "node:assert/strict";
import { beforeEach, test } from "node:test";

import { mockIPC } from "@/ipc/nativeMocks";
import type { Request } from "@/ipc/generated";

const stored = new Map<string, string>();
const localStorage = {
  getItem: (key: string) => stored.get(key) ?? null,
  setItem: (key: string, value: string) => void stored.set(key, value),
  removeItem: (key: string) => void stored.delete(key),
};
Object.defineProperty(globalThis, "localStorage", { configurable: true, value: localStorage });
Object.defineProperty(globalThis, "window", { configurable: true, value: { localStorage, crypto: globalThis.crypto } });

const { handBack, isPaused, useAgentPause, userUsed } = await import("./agentPause");
const { askedOf, dismissSignInRejected, signInCleared, signInSeen, stopAgent, stoppedWords, useSignInRejected } = await import("./browserSignIn");
const { useSessionTabs } = await import("./sessionTabs");

const AGENT = "browser:driven";
const MINE = "browser:mine";
const SIGN_IN = { kind: "signIn", site: "accounts.google.com", key: "signIn https://accounts.google.com" } as const;
const CAPTCHA = { kind: "captcha", site: "shop.example", key: "captcha https://shop.example/cart" } as const;

let told: { tab: string; paused: boolean }[] = [];
let found: unknown = null;
let looks = 0;

mockIPC((command, payload) => {
  if (command === "browser_agent_sign_in_stop") {
    looks += 1;
    if (found instanceof Error) throw found;
    return found;
  }
  const request = (payload as { request?: Request }).request;
  if (request?.method === "browserPaused") told.push({ tab: request.tab, paused: request.paused });
  return { method: request?.method };
});

const browserTab = (id: string, driver: boolean) => ({
  kind: "browser", id, opener: "chat", url: "https://shop.example/", title: "",
  ...(driver ? { agent: true, driver: { label: "Check the shop", taskId: "t1" } } : {}),
});
const front = () => useSessionTabs.getState().sessions.session?.active;
const settle = () => new Promise((resolve) => setImmediate(resolve));

beforeEach(() => {
  useSessionTabs.setState({ sessions: { session: { tabs: [browserTab(AGENT, true), browserTab(MINE, false)], active: "chat" } } } as never);
  useAgentPause.setState({ paused: {}, devtools: {}, asked: {} });
  useSignInRejected.setState({}, true);
  signInCleared(AGENT);
  told = [];
  found = null;
  looks = 0;
});

test("a stop pauses the agent, says why, tells the daemon and brings the tab to the front of its session", () => {
  assert.equal(stopAgent(AGENT, SIGN_IN), true);
  assert.equal(isPaused(AGENT), true);
  assert.deepEqual(askedOf(AGENT), SIGN_IN);
  assert.deepEqual(told, [{ tab: AGENT, paused: true }]);
  assert.equal(front(), AGENT);
});

test("nothing resumes by itself: only Continue ends a stop, and it takes the reason with it", () => {
  stopAgent(AGENT, SIGN_IN);
  signInCleared(AGENT);
  assert.equal(isPaused(AGENT), true, "the page moving on doesn't hand it back");
  handBack(AGENT);
  assert.equal(isPaused(AGENT), false);
  assert.equal(askedOf(AGENT), undefined);
  assert.deepEqual(told, [{ tab: AGENT, paused: true }, { tab: AGENT, paused: false }]);
});

test("after Continue the same stop doesn't stop the tab again, and another one does", () => {
  stopAgent(AGENT, SIGN_IN);
  handBack(AGENT);
  assert.equal(stopAgent(AGENT, SIGN_IN), false);
  assert.equal(isPaused(AGENT), false);
  assert.equal(stopAgent(AGENT, CAPTCHA), true);
  assert.deepEqual(askedOf(AGENT), CAPTCHA);
});

test("a page that needs nobody ends the answer: a later sign-in on the tab is a new one", () => {
  stopAgent(AGENT, SIGN_IN);
  handBack(AGENT);
  signInCleared(AGENT);
  assert.equal(stopAgent(AGENT, SIGN_IN), true);
});

test("the user's own tab has no agent to stop, and a tab they already hold keeps its own words", () => {
  assert.equal(stopAgent(MINE, SIGN_IN), false);
  assert.equal(isPaused(MINE), false);
  assert.equal(front(), "chat");

  userUsed(AGENT);
  told = [];
  assert.equal(stopAgent(AGENT, SIGN_IN), false);
  assert.equal(askedOf(AGENT), undefined, "the strip goes on saying they're using it");
  assert.deepEqual(told, []);
  assert.equal(front(), "chat");
});

test("Google's refusal shows a card until it's dismissed or the page leaves it, on any tab", () => {
  signInSeen(MINE, { rejected: true });
  assert.equal(MINE in useSignInRejected.getState(), true);
  assert.equal(isPaused(MINE), false, "a card, no stop");
  dismissSignInRejected(MINE);
  assert.equal(MINE in useSignInRejected.getState(), false);

  signInSeen(MINE, { rejected: true });
  signInSeen(AGENT, { rejected: true });
  signInSeen(MINE, { rejected: false });
  assert.deepEqual(Object.keys(useSignInRejected.getState()), [AGENT], "and each tab has its own");
});

test("a stop main found, in a window of the tab, stops the tab's agent", () => {
  const stop = { ...SIGN_IN, window: true };
  signInSeen(AGENT, { stop });
  assert.deepEqual(askedOf(AGENT), stop);
});

test("a look goes to the page only when an agent drives the tab", async () => {
  signInSeen(MINE, { look: true });
  await settle();
  assert.equal(looks, 0, "the user's own page isn't looked into");

  userUsed(AGENT);
  signInSeen(AGENT, { look: true });
  await settle();
  assert.equal(looks, 0, "nor one the user has taken over");
  handBack(AGENT);

  found = SIGN_IN;
  signInSeen(AGENT, { look: true });
  await settle();
  assert.equal(looks, 1);
  assert.deepEqual(askedOf(AGENT), SIGN_IN);
});

test("a look that finds nothing ends the answer, and one that fails stops nothing", async () => {
  stopAgent(AGENT, SIGN_IN);
  handBack(AGENT);
  signInSeen(AGENT, { look: true });
  await settle();
  assert.equal(stopAgent(AGENT, SIGN_IN), true, "the page left the sign-in, so this is a new one");
  handBack(AGENT);

  const logged = console.error;
  console.error = () => {};
  try {
    found = new Error("no such tab");
    signInSeen(AGENT, { look: true });
    await settle();
    assert.equal(isPaused(AGENT), false);
  } finally {
    console.error = logged;
  }
});

test("what the agent hears says why it stopped, that nothing more was done, and that its next call waits", () => {
  const words = (stop: Parameters<typeof stoppedWords>[0]) => stoppedWords(stop, AGENT).agent;
  assert.match(words(SIGN_IN), /^Stopped: accounts\.google\.com is asking for a sign-in\. Agents don't sign in: only the user does\./);
  assert.match(words(CAPTCHA), /^Stopped: shop\.example shows a captcha\. Agents never answer these, and only the user can\./);
  assert.match(words({ kind: "rejected", site: "accounts.google.com", key: "r" }), /^Stopped: Google refused this sign-in for this browser \(accounts\.google\.com\)\./);
  assert.match(words({ ...SIGN_IN, window: true }), /^Stopped: this page opened a separate sign-in window at accounts\.google\.com\. You can't see or use that window/);
  for (const stop of [SIGN_IN, CAPTCHA]) {
    assert.match(words(stop), new RegExp(`The user has this tab now and nothing more was done here\\. Tell them what you need\\. Your next call on ${AGENT} waits until they press Continue\\.$`));
  }
});
