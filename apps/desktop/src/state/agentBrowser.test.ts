import assert from "node:assert/strict";
import { test } from "node:test";
import { mockIPC } from "@tauri-apps/api/mocks";
import type { Channel } from "@tauri-apps/api/core";
import type { AppCall, BrowserEvent, BrowserOp, BrowserResult, Request } from "@/ipc/generated";

const data = new Map<string, string>();
Object.defineProperty(globalThis, "localStorage", { configurable: true, value: {
  getItem: (key: string) => data.get(key) ?? null,
  setItem: (key: string, value: string) => data.set(key, value),
  removeItem: (key: string) => data.delete(key),
  get length() { return data.size; },
  key: (at: number) => [...data.keys()][at] ?? null,
} });
Object.defineProperty(globalThis, "window", { configurable: true, value: { localStorage, crypto: globalThis.crypto, innerWidth: 1200, innerHeight: 800 } });
const { base64 } = await import("./agentBrowser");
const { runAppCall } = await import("./appCalls");
const { handBack, isPaused, pageShows, userUsed } = await import("./agentPause");
const { useBrowsers } = await import("./browsers");
const { CHAT_TAB, selectTab, sessionTabs } = await import("./sessionTabs");
const { useApp } = await import("./store");
// Closing a tab closes its page.
await import("./sessionTabResources");

const SESSION = "agent-browser-session";

/** The shell and daemon, faked: pages load at once, the script answers as `answer` says. */
function fakeShell(answer: (op: Record<string, unknown>) => unknown) {
  const channels = new Map<string, Channel<BrowserEvent>>();
  const opened: { id: string; url: string; agent: boolean }[] = [];
  const shown: string[] = [];
  const scripts: { id: string; op: Record<string, unknown> }[] = [];
  const answers: { id: string; result: BrowserResult }[] = [];
  const loads: string[] = [];
  const paused: { tab: string; paused: boolean }[] = [];
  const load = (id: string, url: string) => {
    const events = channels.get(id)!;
    events.onmessage({ type: "load", url, loading: true });
    setTimeout(() => events.onmessage({ type: "load", url, loading: false }), 5);
  };
  mockIPC((command, payload) => {
    const args = payload as { id: string; url: string; agent?: boolean; events?: Channel<BrowserEvent>; op: Record<string, unknown>; bounds?: unknown; request?: Request };
    switch (command) {
      case "browser_open":
        channels.set(args.id, args.events!);
        opened.push({ id: args.id, url: args.url, agent: args.agent ?? false });
        setTimeout(() => load(args.id, args.url), 1);
        return null;
      case "browser_place":
        if (args.bounds) shown.push(args.id);
        return null;
      case "browser_agent_load":
        loads.push(args.url);
        setTimeout(() => load(args.id, args.url), 1);
        return null;
      case "browser_go":
        setTimeout(() => load(args.id, "http://127.0.0.1:4000/"), 1);
        return null;
      case "browser_agent":
        scripts.push({ id: args.id, op: args.op });
        return answer(args.op);
      case "browser_agent_capture":
        return new Uint8Array([137, 80, 78, 71]).buffer;
      case "ipc_request": {
        const sent = args.request as Extract<Request, { method: "appAnswer" }>;
        if (sent.method === "appAnswer")
          answers.push({ id: sent.id, result: sent.result.app === "browser" ? sent.result.result : { result: "failed", message: sent.result.app === "failed" ? sent.result.message : sent.result.app } });
        const told = args.request as Request;
        if (told.method === "browserPaused") paused.push({ tab: told.tab, paused: told.paused });
        return { method: sent.method };
      }
      default:
        return null;
    }
  });
  return { opened, shown, scripts, answers, loads, paused, channels };
}

const reads = (op: Record<string, unknown>) =>
  op.kind === "read" ? { url: "http://127.0.0.1:4000/", title: "Shop", text: '[e1] button "Buy"' } : { done: true };

let calls = 0;
function call(op: BrowserOp): AppCall {
  return { id: `app-call-${++calls}`, conversationId: SESSION, taskId: null, label: "Check the shop", op: { app: "browser", op } };
}

async function answered(shell: { answers: { id: string; result: BrowserResult }[] }, op: BrowserOp): Promise<BrowserResult> {
  const sent = call(op);
  await runAppCall(sent);
  const answer = shell.answers.find((each) => each.id === sent.id);
  assert.ok(answer, `${sent.id} was answered`);
  return answer.result;
}

function tabOf(result: BrowserResult): string {
  assert.equal(result.result, "page", JSON.stringify(result));
  return (result as Extract<BrowserResult, { result: "page" }>).tab;
}

test("an agent's tab opens behind the one in front, and works there without ever being shown", async () => {
  useApp.setState((state) => ({ settings: { ...state.settings, browserEnabled: true } }));
  const shell = fakeShell(reads);
  const opened = await answered(shell, { op: "open", url: "http://127.0.0.1:4000/" });
  const tab = tabOf(opened);
  assert.deepEqual(opened, {
    result: "page", tab,
    page: { url: "http://127.0.0.1:4000/", title: "Shop", generation: 1, loading: false, text: '[e1] button "Buy"' },
  });
  const tabs = sessionTabs(SESSION);
  assert.equal(tabs.active, CHAT_TAB, "the user stays where they were");
  assert.deepEqual(tabs.tabs.map((each) => each.kind === "browser" && [each.id, each.agent, each.driver]), [
    [tab, true, { label: "Check the shop", taskId: null }],
  ]);
  assert.deepEqual(shell.opened, [{ id: tab, url: "http://127.0.0.1:4000/", agent: true }], "a normal Browser tab's page, its pages the agent's in the history");

  // Read, click, type and a picture, on the tab the user never chose.
  assert.equal((await answered(shell, { op: "read", tab })).result, "page");
  assert.equal((await answered(shell, { op: "click", tab, target: "e1", guard: { declared: null, approved: null } })).result, "page");
  assert.equal((await answered(shell, { op: "type", tab, target: "e2", text: "mug", submit: true, guard: { declared: null, approved: null } })).result, "page");
  assert.deepEqual(await answered(shell, { op: "screenshot", tab }), { result: "shot", tab, url: "http://127.0.0.1:4000/", png: "iVBORw==" });
  assert.deepEqual(shell.scripts.map((each) => each.op.kind), ["read", "read", "click", "read", "type", "read"]);
  assert.deepEqual(shell.scripts[4]!.op, { kind: "type", target: "e2", text: "mug", submit: true, declared: null, approved: null });
  assert.deepEqual(shell.shown, [], "the page was never put over a tab");

  const navigated = await answered(shell, { op: "navigate", tab, url: "http://127.0.0.1:4000/cart" });
  assert.deepEqual(shell.loads, ["http://127.0.0.1:4000/cart"]);
  assert.equal(navigated.result === "page" && navigated.page.generation, 2, "each load is a new page");

  assert.deepEqual(await answered(shell, { op: "close", tab }), { result: "closed", tab });
  assert.deepEqual(sessionTabs(SESSION).tabs, []);
  assert.equal(useBrowsers.getState().pages[tab], undefined, "its page went with it");
});

test("an action that asks first does nothing, and a yes holds only on the page it was given on", async () => {
  useApp.setState((state) => ({ settings: { ...state.settings, browserEnabled: true } }));
  const shell = fakeShell((op) =>
    op.kind === "read" ? reads(op)
      : op.approved === "http://127.0.0.1:4000/" ? { done: true }
      : { ask: { sensitive: "credentials", what: 'textbox "Password"' }, url: "http://127.0.0.1:4000/" });
  const tab = tabOf(await answered(shell, { op: "open", url: "http://127.0.0.1:4000/" }));
  const typing = (approved: { generation: number; url: string } | null): BrowserOp =>
    ({ op: "type", tab, target: "e3", text: "hunter2", submit: false, guard: { declared: null, approved } });

  assert.deepEqual(await answered(shell, typing(null)), {
    result: "needsApproval", tab, url: "http://127.0.0.1:4000/", generation: 1, sensitive: "credentials", what: 'textbox "Password"',
  });
  assert.equal((await answered(shell, typing({ generation: 1, url: "http://127.0.0.1:4000/" }))).result, "page", "with the yes it goes");

  // The page loaded again since: the yes is void, and the agent is asked about it again.
  await answered(shell, { op: "go", tab, action: "reload" });
  const again = await answered(shell, typing({ generation: 1, url: "http://127.0.0.1:4000/" }));
  assert.equal(again.result, "needsApproval");
  assert.equal(shell.scripts.at(-1)!.op.approved, null);
  selectTab(SESSION, CHAT_TAB);
});

test("the user's input in an agent's tab pauses it until Continue, and closing the tab hands it back", async () => {
  useApp.setState((state) => ({ settings: { ...state.settings, browserEnabled: true } }));
  const shell = fakeShell(reads);
  const tab = tabOf(await answered(shell, { op: "open", url: "http://127.0.0.1:4000/" }));
  const click: BrowserOp = { op: "click", tab, target: "e1", guard: { declared: null, approved: null } };

  // Input its page hears while behind its tab is none of the user's (#416).
  shell.channels.get(tab)!.onmessage({ type: "userInput" });
  assert.equal(isPaused(tab), false);
  assert.deepEqual(shell.paused, []);
  // Its tab in front, a click in its page, as the page's input script tells it.
  pageShows(tab, true);
  shell.channels.get(tab)!.onmessage({ type: "userInput" });
  assert.equal(isPaused(tab), true);
  assert.deepEqual(shell.paused, [{ tab, paused: true }], "the daemon holds the agent's calls");
  const before = shell.scripts.length;
  assert.deepEqual(await answered(shell, click), { result: "paused", tab });
  assert.equal(shell.scripts.length, before, "nothing was done on the page");

  // More input, and the page loading again, change nothing; Continue hands it back.
  userUsed(tab);
  shell.channels.get(tab)!.onmessage({ type: "load", url: "http://127.0.0.1:4000/next", loading: true });
  shell.channels.get(tab)!.onmessage({ type: "load", url: "http://127.0.0.1:4000/next", loading: false });
  assert.equal(isPaused(tab), true, "the pause lives outside the page");
  handBack(tab);
  assert.deepEqual(shell.paused, [{ tab, paused: true }, { tab, paused: false }]);
  assert.equal((await answered(shell, click)).result, "page");

  // Paused again, then closed: the agent's calls on it go on (and hear it is gone).
  userUsed(tab);
  assert.deepEqual(await answered(shell, { op: "close", tab }), { result: "paused", tab }, "not even the agent closes it while paused");
  handBack(tab);
  userUsed(tab);
  const { closeTab } = await import("./sessionTabs");
  closeTab(SESSION, tab);
  assert.equal(isPaused(tab), false);
  assert.deepEqual(shell.paused.slice(-2), [{ tab, paused: true }, { tab, paused: false }]);
});

test("the user's own Browser tabs never pause anything", async () => {
  const shell = fakeShell(reads);
  const { openBrowserTab } = await import("./sessionTabs");
  const id = openBrowserTab(SESSION, "http://127.0.0.1:4000/");
  userUsed(id);
  assert.equal(isPaused(id), false);
  assert.deepEqual(shell.paused, []);
});

test("calls on a tab that isn't an agent's, or with the browser off, fail in plain words", async () => {
  const shell = fakeShell(reads);
  useApp.setState((state) => ({ settings: { ...state.settings, browserEnabled: true } }));
  assert.deepEqual(await answered(shell, { op: "read", tab: "browser:someone-else" }), {
    result: "failed", message: "There is no tab browser:someone-else of yours in this session now: open one with browser_open.",
  });
  useApp.setState((state) => ({ settings: { ...state.settings, browserEnabled: false } }));
  assert.deepEqual(await answered(shell, { op: "open", url: "http://127.0.0.1:4000/" }), { result: "failed", message: "The built-in browser is off." });
  useApp.setState((state) => ({ settings: { ...state.settings, browserEnabled: true } }));
});

test("a click that opens a popup names its tab, and the agent works there", async () => {
  useApp.setState((state) => ({ settings: { ...state.settings, browserEnabled: true } }));
  const { openPopupTab } = await import("./sessionTabs");
  let opener = "";
  const shell = fakeShell((op) => {
    // The page's sign-in button opens a window, as the shell reports one: its page made, a tab next.
    if (op.kind === "click") openPopupTab(opener, "browser:popup-1", "http://127.0.0.1:4000/sign-in");
    return reads(op);
  });
  opener = tabOf(await answered(shell, { op: "open", url: "http://127.0.0.1:4000/" }));
  const clicked = await answered(shell, { op: "click", tab: opener, target: "e1", guard: { declared: null, approved: null } });
  assert.equal(clicked.result === "page" && clicked.page.text,
    '[e1] button "Buy"\n\n(This opened a new tab: browser:popup-1 at http://127.0.0.1:4000/sign-in. Use its id with the browser tools.)');
  assert.equal(sessionTabs(SESSION).active, CHAT_TAB, "it opened behind, like its opener");
  useBrowsers.setState(({ pages }) => ({ pages: { ...pages, "browser:popup-1": { ...pages[opener]!, url: "http://127.0.0.1:4000/sign-in" } } }));
  assert.equal(tabOf(await answered(shell, { op: "read", tab: "browser:popup-1" })), "browser:popup-1");
  const again = await answered(shell, { op: "click", tab: opener, target: "e1", guard: { declared: null, approved: null } });
  assert.equal(again.result === "page" && again.page.text, '[e1] button "Buy"', "only a tab this action opened is named");
});

test("a picture's bytes go as base64, however many", () => {
  assert.equal(base64(new Uint8Array([104, 105])), "aGk=");
  const big = new Uint8Array(100_000).fill(65);
  assert.equal(atob(base64(big)).length, 100_000);
});
