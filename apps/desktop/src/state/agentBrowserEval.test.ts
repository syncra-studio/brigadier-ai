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
const { devToolsChanged } = await import("./agentPause");
const { useBrowsers } = await import("./browsers");
const { closeTab, openBrowserTab, sessionTabs } = await import("./sessionTabs");
const { useApp } = await import("./store");
await import("./sessionTabResources");

const SESSION = "agent-eval-session";
const SHOP = "http://127.0.0.1:4000/";

const ran: unknown[] = [];
const answers = new Map<string, BrowserResult>();
const channels = new Map<string, Channel<BrowserEvent>>();

function load(id: string, url: string): void {
  const events = channels.get(id)!;
  events.onmessage({ type: "load", url, loading: true });
  setTimeout(() => events.onmessage({ type: "load", url, loading: false }), 2);
}

mockIPC((command, payload) => {
  const args = payload as { id: string; url: string; events?: Channel<BrowserEvent>; op: { kind: string; script?: string }; request?: Request };
  switch (command) {
    case "browser_open":
      channels.set(args.id, args.events!);
      setTimeout(() => load(args.id, args.url), 1);
      return null;
    case "browser_agent_load":
      setTimeout(() => load(args.id, args.url), 1);
      return null;
    case "browser_agent":
      if (args.op.kind === "read") return { url: SHOP, title: "Shop", text: "" };
      ran.push(args.op);
      return { text: "42", url: SHOP };
    case "ipc_request": {
      const request = args.request as Request;
      if (request.method === "appAnswer" && request.result.app === "browser") answers.set(request.id, request.result.result);
      return { method: request.method };
    }
    default:
      return null;
  }
});

let calls = 0;
async function answered(op: BrowserOp): Promise<BrowserResult> {
  const sent: AppCall = { id: `eval-call-${++calls}`, conversationId: SESSION, taskId: null, label: "Check", op: { app: "browser", op } };
  await runAppCall(sent);
  const answer = answers.get(sent.id);
  assert.ok(answer, `${sent.id} was answered`);
  return answer;
}

async function agentTab(): Promise<string> {
  const opened = await answered({ op: "open", url: SHOP, keep: false, request: null });
  assert.equal(opened.result, "page", JSON.stringify(opened));
  return (opened as Extract<BrowserResult, { result: "page" }>).tab;
}

const generationOf = (tab: string) => useBrowsers.getState().pages[tab]?.generation ?? 0;
const evalOp = (tab: string, approved: { generation: number; url: string } | null): BrowserOp => ({
  op: "eval",
  tab,
  script: "return 42",
  guard: { declared: null, approved },
});

beforeEach(() => {
  useApp.setState((state) => ({ settings: { ...state.settings, browserEnabled: true } }));
  for (const tab of sessionTabs(SESSION).tabs) closeTab(SESSION, tab.id);
  ran.length = 0;
});

test("a script asks first and nothing runs", async () => {
  const tab = await agentTab();
  assert.deepEqual(await answered(evalOp(tab, null)), {
    result: "needsApproval", tab, url: SHOP, generation: generationOf(tab), sensitive: "script", what: "",
  });
  assert.deepEqual(ran, []);
});

test("with the yes given on this page it runs, and answers as text", async () => {
  const tab = await agentTab();
  const result = await answered(evalOp(tab, { generation: generationOf(tab), url: SHOP }));
  assert.deepEqual(result, { result: "evaluated", tab, url: SHOP, text: "42" });
  assert.deepEqual(ran, [{ kind: "eval", script: "return 42" }]);
});

test("a yes given for another address or an earlier page asks again", async () => {
  const tab = await agentTab();
  const generation = generationOf(tab);
  assert.equal((await answered(evalOp(tab, { generation, url: "http://127.0.0.1:4000/other" }))).result, "needsApproval");
  assert.equal((await answered(evalOp(tab, { generation: generation - 1, url: SHOP }))).result, "needsApproval");
  assert.deepEqual(ran, []);
});

test("a script can't run on the user's own tab", async () => {
  const tab = openBrowserTab(SESSION, SHOP, false);
  const id = typeof tab === "string" ? tab : sessionTabs(SESSION).tabs.at(-1)!.id;
  const result = await answered(evalOp(id, { generation: 0, url: SHOP }));
  assert.equal(result.result, "failed");
  assert.match((result as { message: string }).message, /is the user's: you may read it/);
  assert.deepEqual(ran, []);
});

test("with DevTools open on the tab a script gets devtools_open", async () => {
  const tab = await agentTab();
  devToolsChanged(tab, true);
  try {
    const result = await answered(evalOp(tab, { generation: generationOf(tab), url: SHOP }));
    assert.equal(result.result, "failed");
    assert.match((result as { message: string }).message, /^devtools_open:/);
    assert.deepEqual(ran, []);
  } finally {
    devToolsChanged(tab, false);
  }
});
