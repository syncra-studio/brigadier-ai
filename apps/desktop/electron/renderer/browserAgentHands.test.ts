import assert from "node:assert/strict";
import test from "node:test";

import "../testing/fakeElectron.ts";

// The little of a document the tab layer touches: a <webview> is ready as soon as it's in the layer.
class FakeElement {
  tagName: string;
  style: Record<string, string> = {};
  dataset: Record<string, string> = {};
  isConnected = false;
  private listeners = new Map<string, ((event: unknown) => void)[]>();
  guest = 0;
  constructor(tag: string) { this.tagName = tag.toUpperCase(); }
  setAttribute() {}
  toggleAttribute() {}
  blur() {}
  focus() {}
  remove() {}
  after() {}
  prepend(child: FakeElement) { this.append(child); }
  append(...children: (FakeElement | string)[]) {
    for (const child of children) {
      if (typeof child === "string") continue;
      child.isConnected = true;
      child.fire("dom-ready");
    }
  }
  addEventListener(name: string, listener: (event: unknown) => void) { this.listeners.set(name, [...(this.listeners.get(name) ?? []), listener]); }
  fire(name: string, event: unknown = {}) {
    const each = this.listeners.get(name) ?? [];
    this.listeners.delete(name);
    for (const listener of each) listener(event);
  }
  getWebContentsId() { return this.guest; }
  getZoomFactor() { return 1; }
  canGoBack() { return false; }
  canGoForward() { return false; }
  executeJavaScript() { return Promise.resolve(null); }
}

let nextGuest = 700;
const made: FakeElement[] = [];
const body = new FakeElement("body");
const globals = globalThis as Record<string, unknown>;
globals.HTMLElement = FakeElement;
globals.Element = FakeElement;
globals.MutationObserver = class { observe() {} };
globals.matchMedia = () => ({ matches: true });
globals.document = {
  body,
  head: new FakeElement("head"),
  activeElement: body,
  hasFocus: () => true,
  addEventListener: () => {},
  getElementById: () => null,
  createElement: (tag: string) => {
    const element = new FakeElement(tag);
    if (tag === "webview") {
      element.guest = nextGuest++;
      made.push(element);
    }
    return element;
  },
};

type Heard = { from: number } & Record<string, unknown>;
const listeners: Record<string, (details: Heard) => void> = {};
const agentCalls: { call: string; shows: string }[] = [];
let answerScript: (op: Record<string, unknown>) => unknown = () => ({ point: { x: 40, y: 20 } });
let duringInput: () => void = () => {};
const stored = new Map<string, string>();
const localStorage = { getItem: (key: string) => stored.get(key) ?? null, setItem: (key: string, value: string) => void stored.set(key, value), removeItem: (key: string) => void stored.delete(key) };
Object.defineProperty(globalThis, "localStorage", { configurable: true, value: localStorage });

const shell = new Proxy({
  profilePartition: async () => "persist:browser-profile-default",
  agent: async (call: { call: string; guest: number; op?: Record<string, unknown> }) => {
    const element = made.find((each) => each.guest === call.guest);
    const name = call.call === "script" ? `script ${String(call.op?.kind)}` : call.call;
    agentCalls.push({ call: name, shows: element?.style.visibility ?? "" });
    if (call.call === "script") return answerScript(call.op!);
    if (call.call === "click" || call.call === "enter") duringInput();
    if (call.call === "signInStop") return { kind: "signIn", site: "site.test", key: "signIn https://site.test" };
    return { done: true };
  },
}, {
  get: (known, name: string) => {
    if (name in known) return known[name as keyof typeof known];
    // Each of the shell's onGuest…: the test speaks for main.
    return (listener: (details: Heard) => void) => { listeners[name] = listener; };
  },
});
globals.window = {
  localStorage,
  crypto: globalThis.crypto,
  addEventListener: () => {},
  brigadierShell: shell,
  brigadierNative: {
    invoke: async (command: string, args: { request?: { method: string } }) => ({ value: command === "ipc_request" ? { method: args.request?.method } : null }),
    onChannel: () => {},
    listen: () => () => {},
  },
};

const { installTabLayer, openPage, placePage } = await import("./tabLayer.ts");
const { agentRun } = await import("./browserAgentHands.ts");
const { useAgentPause } = await import("@/state/agentPause");
const { useSignInRejected } = await import("@/state/browserSignIn");
const { useSessionTabs } = await import("@/state/sessionTabs");

installTabLayer();

const BOUNDS = { x: 10, y: 10, width: 800, height: 600 };
let opened = 0;

/** A tab's page, behind its tab as an agent's opens. */
async function background() {
  const id = `browser:hands-${++opened}`;
  const events: unknown[] = [];
  await openPage(id, "https://shop.example/", BOUNDS, (event) => events.push(event));
  agentCalls.length = 0;
  return { id, element: made.at(-1)!, events };
}

const shownAt = (call: string) => agentCalls.find((each) => each.call === call)?.shows;

test("an agent's click reaches a page behind its tab, shown only for that long", async () => {
  const { id, element } = await background();
  assert.equal(element.style.visibility, "hidden");
  assert.deepEqual(await agentRun(id, { kind: "click", target: "e1", label: "An agent" }), { done: true });
  // Input sent to a page with visibility:hidden reaches nothing, though the call says it succeeded.
  assert.equal(shownAt("click"), "visible");
  assert.equal(element.style.visibility, "hidden", "and it goes back behind its tab");
});

test("an agent's Enter does too, but the typing needs no showing", async () => {
  const { id, element } = await background();
  answerScript = (op) => op.kind === "insert" ? { done: true, typed: true } : { point: { x: 40, y: 20 } };
  await agentRun(id, { kind: "type", target: "e1", text: "hello", submit: true, label: "An agent" });
  assert.equal(shownAt("enter"), "visible");
  assert.equal(shownAt("script insert"), "hidden");
  assert.equal(element.style.visibility, "hidden");

  // No submit: nothing to press, so nothing shows.
  agentCalls.length = 0;
  await agentRun(id, { kind: "type", target: "e1", text: "hello", label: "An agent" });
  assert.equal(shownAt("enter"), undefined);
  answerScript = () => ({ point: { x: 40, y: 20 } });
});

test("a tab in front stays shown through the click, and a tab brought to the front during one stays in front", async () => {
  const front = await background();
  placePage(front.id, BOUNDS);
  await agentRun(front.id, { kind: "click", target: "e1", label: "An agent" });
  assert.equal(shownAt("click"), "visible");
  assert.equal(front.element.style.visibility, "visible");

  const lent = await background();
  duringInput = () => placePage(lent.id, BOUNDS);
  await agentRun(lent.id, { kind: "click", target: "e1", label: "An agent" });
  duringInput = () => {};
  assert.equal(lent.element.style.visibility, "visible");
});

test("a page behind its tab goes back even when the click fails", async () => {
  const { id, element } = await background();
  duringInput = () => { throw new Error("debugger gone"); };
  await assert.rejects(agentRun(id, { kind: "click", target: "e1", label: "An agent" }), /debugger gone/);
  duringInput = () => {};
  assert.equal(element.style.visibility, "hidden");
});

test("what main says of a guest reaches its tab's banner, and no other tab's", async () => {
  const { id, element, events } = await background();
  const other = await background();

  listeners.onGuestPasskey!({ from: element.guest });
  assert.deepEqual(events.filter((each) => (each as { type: string }).type === "passkeyRefused"), [{ type: "passkeyRefused" }]);
  assert.deepEqual(other.events.filter((each) => (each as { type: string }).type === "passkeyRefused"), []);

  listeners.onGuestSignIn!({ from: element.guest, rejected: true });
  assert.equal(id in useSignInRejected.getState(), true);
  assert.equal(other.id in useSignInRejected.getState(), false);
  listeners.onGuestSignIn!({ from: element.guest, rejected: false });
  assert.equal(id in useSignInRejected.getState(), false);
});

test("a window's sign-in that main found stops the agent driving the tab that opened it", async () => {
  const { id, element } = await background();
  useSessionTabs.setState({ sessions: { session: { tabs: [{ kind: "browser", id, opener: "chat", url: "https://shop.example/", title: "", agent: true, driver: { label: "An agent", taskId: "t1" } }], active: "chat" } } } as never);
  const stop = { kind: "signIn", site: "accounts.google.com", key: "signIn https://accounts.google.com", window: true };
  listeners.onGuestSignIn!({ from: element.guest, stop });
  assert.deepEqual(useAgentPause.getState().asked[id], stop);
});
