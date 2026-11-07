import assert from "node:assert/strict";
import { beforeEach, test } from "node:test";

import { mockIPC } from "@/ipc/nativeMocks";
import type { Channel } from "@/ipc/native";
import type { AppCall, BrowserEvent, BrowserOp, BrowserResult, Request } from "@/ipc/generated";

const stored = new Map<string, string>();
const localStorage = {
  getItem: (key: string) => stored.get(key) ?? null,
  setItem: (key: string, value: string) => void stored.set(key, value),
  removeItem: (key: string) => void stored.delete(key),
};
Object.defineProperty(globalThis, "localStorage", { configurable: true, value: localStorage });
Object.defineProperty(globalThis, "window", { configurable: true, value: { localStorage, crypto: globalThis.crypto, innerWidth: 1200, innerHeight: 800 } });

const { runAppCall } = await import("./appCalls");
const { handBack, isPaused, useAgentPause, userUsed } = await import("./agentPause");
const { signInCleared, signInSeen } = await import("./browserSignIn");
const { closeTab, openBrowserTab, sessionTabs } = await import("./sessionTabs");
const { useApp } = await import("./store");
await import("./sessionTabResources");

const SESSION = "agent-sign-in-session";
const SHOP = "http://127.0.0.1:4000/";
const SIGN_IN = { kind: "signIn", site: "accounts.google.com", key: "signIn https://accounts.google.com" };

let stopHere: unknown = null;
let landing: (() => void) | null = null;
const ran: string[] = [];
const paused: { tab: string; paused: boolean }[] = [];
const answers = new Map<string, BrowserResult>();
const channels = new Map<string, Channel<BrowserEvent>>();

function load(id: string, url: string): void {
  const events = channels.get(id)!;
  events.onmessage({ type: "load", url, loading: true });
  setTimeout(() => events.onmessage({ type: "load", url, loading: false }), 2);
}

mockIPC((command, payload) => {
  const args = payload as { id: string; url: string; events?: Channel<BrowserEvent>; op: { kind: string }; request?: Request };
  switch (command) {
    case "browser_open":
      channels.set(args.id, args.events!);
      setTimeout(() => load(args.id, args.url), 1);
      return null;
    case "browser_agent_load":
      setTimeout(() => load(args.id, args.url), 1);
      return null;
    case "browser_agent":
      ran.push(args.op.kind);
      if (args.op.kind === "click") landing?.();
      return args.op.kind === "read" ? { url: SHOP, title: "Shop", text: '[e1] button "Sign in"' } : { done: true };
    case "browser_agent_sign_in_stop":
      return stopHere;
    case "ipc_request": {
      const request = args.request as Request;
      if (request.method === "appAnswer" && request.result.app === "browser") answers.set(request.id, request.result.result);
      if (request.method === "browserPaused") paused.push({ tab: request.tab, paused: request.paused });
      return { method: request.method };
    }
    default:
      return null;
  }
});

let calls = 0;
async function answered(op: BrowserOp): Promise<BrowserResult> {
  const sent: AppCall = { id: `sign-in-call-${++calls}`, conversationId: SESSION, taskId: null, label: "Check the shop", op: { app: "browser", op } };
  await runAppCall(sent);
  const answer = answers.get(sent.id);
  assert.ok(answer, `${sent.id} was answered`);
  return answer;
}

const open = (url = SHOP): BrowserOp => ({ op: "open", url, keep: false, request: null });
const click = (tab: string): BrowserOp => ({ op: "click", tab, target: "e1", guard: { declared: null, approved: null } });
const read = (tab: string): BrowserOp => ({ op: "read", tab });

function textOf(result: BrowserResult): string {
  assert.equal(result.result, "page", JSON.stringify(result));
  return (result as Extract<BrowserResult, { result: "page" }>).page.text;
}

async function tabAtShop(): Promise<string> {
  const result = await answered(open());
  assert.equal(result.result, "page");
  ran.length = 0;
  paused.length = 0;
  return (result as Extract<BrowserResult, { result: "page" }>).tab;
}

const times = (kind: string) => ran.filter((each) => each === kind).length;

useApp.setState((state) => ({ settings: { ...state.settings, browserEnabled: true } }));

beforeEach(() => {
  for (const tab of sessionTabs(SESSION).tabs) closeTab(SESSION, tab.id);
  useAgentPause.setState({ paused: {}, devtools: {}, asked: {} });
  stopHere = null;
  landing = null;
  ran.length = 0;
  paused.length = 0;
});

test("a click that lands on a sign-in answers Stopped, not `paused`: the click is done, and the page isn't read", async () => {
  const tab = await tabAtShop();
  landing = () => { stopHere = SIGN_IN; };
  const result = await answered(click(tab));
  assert.match(textOf(result), /^Stopped: accounts\.google\.com is asking for a sign-in\./);
  assert.match(textOf(result), new RegExp(`Your next call on ${tab} waits until they press Continue\\.$`));
  assert.equal(times("click"), 1);
  assert.equal(times("read"), 0, "the agent hears nothing of the sign-in page");
  assert.equal(isPaused(tab), true);
  assert.deepEqual(paused, [{ tab, paused: true }]);
  assert.equal(sessionTabs(SESSION).active, tab, "the tab is the user's now, so they see it");
  signInCleared(tab);
});

test("no double run: the daemon runs `paused` calls again after Continue, so the stopped click must not say it", async () => {
  const tab = await tabAtShop();
  landing = () => { stopHere = SIGN_IN; };

  // As the daemon does: a call that heard `paused` waits for Continue and goes again.
  const daemon = async (op: BrowserOp): Promise<BrowserResult> => {
    const first = await answered(op);
    if (first.result !== "paused") return first;
    handBack(tab);
    return answered(op);
  };
  const stopped = await daemon(click(tab));
  assert.match(textOf(stopped), /^Stopped:/);
  handBack(tab);
  landing = null;
  assert.equal(times("click"), 1, "the one click");

  // The next call goes on where the user left it, not stopped at the same sign-in a second time.
  assert.match(textOf(await answered(read(tab))), /Sign in/);
  assert.equal(isPaused(tab), false);
  assert.equal(times("click"), 1);
  assert.deepEqual(paused, [{ tab, paused: true }, { tab, paused: false }]);
  signInCleared(tab);
});

test("while stopped, the agent's next call does nothing and answers `paused`", async () => {
  const tab = await tabAtShop();
  stopHere = SIGN_IN;
  assert.match(textOf(await answered(read(tab))), /^Stopped:/);
  ran.length = 0;
  assert.deepEqual(await answered(click(tab)), { result: "paused", tab });
  assert.deepEqual(ran, []);
  handBack(tab);
  signInCleared(tab);
});

test("opening or navigating to a sign-in stops too", async () => {
  stopHere = SIGN_IN;
  const opened = await answered(open("https://accounts.google.com/"));
  assert.match(textOf(opened), /^Stopped:/);
  const tab = (opened as Extract<BrowserResult, { result: "page" }>).tab;
  handBack(tab);
  signInCleared(tab);

  stopHere = null;
  const second = await tabAtShop();
  stopHere = SIGN_IN;
  assert.match(textOf(await answered({ op: "navigate", tab: second, url: "https://accounts.google.com/" })), /^Stopped:/);
  handBack(second);
  signInCleared(second);
});

test("a stop that came while the click settled is the answer, with no second look", async () => {
  const tab = await tabAtShop();
  const stop = { ...SIGN_IN, window: true };
  landing = () => signInSeen(tab, { stop });
  stopHere = null;
  assert.match(textOf(await answered(click(tab))), /^Stopped: this page opened a separate sign-in window at accounts\.google\.com\./);
  handBack(tab);
  signInCleared(tab);
});

test("a page that stops being one ends the answer: coming back to the sign-in stops again", async () => {
  const tab = await tabAtShop();
  stopHere = SIGN_IN;
  assert.match(textOf(await answered(read(tab))), /^Stopped:/);
  handBack(tab);
  assert.doesNotMatch(textOf(await answered(read(tab))), /^Stopped:/, "still there, and Continue answered it");
  stopHere = null;
  assert.doesNotMatch(textOf(await answered(read(tab))), /^Stopped:/);
  stopHere = SIGN_IN;
  assert.match(textOf(await answered(read(tab))), /^Stopped:/);
  handBack(tab);
  signInCleared(tab);
});

test("a tab the user took over themselves stays `paused`, as ever: they hold it and nothing was done", async () => {
  const tab = await tabAtShop();
  stopHere = SIGN_IN;
  userUsed(tab);
  assert.deepEqual(await answered(read(tab)), { result: "paused", tab });
  handBack(tab);
  signInCleared(tab);
});

test("the user's own tab is read as it is: no agent drives it, so its sign-in stops nothing", async () => {
  stopHere = SIGN_IN;
  const tab = openBrowserTab(SESSION, SHOP);
  await new Promise((resolve) => setTimeout(resolve, 30));
  ran.length = 0;
  const result = await answered(read(tab));
  assert.equal(result.result, "page", JSON.stringify(result));
  assert.equal(isPaused(tab), false);
  assert.equal(times("read"), 1);
});
