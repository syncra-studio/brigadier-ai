import assert from "node:assert/strict";
import { test } from "node:test";
import { mockIPC } from "@tauri-apps/api/mocks";
import type { Channel } from "@tauri-apps/api/core";
import type { AppAnswer, AppCall, AppOp, BrowserEvent, Request } from "@/ipc/generated";

const data = new Map<string, string>();
Object.defineProperty(globalThis, "localStorage", { configurable: true, value: {
  getItem: (key: string) => data.get(key) ?? null,
  setItem: (key: string, value: string) => data.set(key, value),
  removeItem: (key: string) => data.delete(key),
  get length() { return data.size; },
  key: (at: number) => [...data.keys()][at] ?? null,
} });
Object.defineProperty(globalThis, "window", { configurable: true, value: { localStorage, crypto: globalThis.crypto, innerWidth: 1200, innerHeight: 800 } });
const { runAppCall } = await import("./appCalls");
const { newSessionTab, openAgentTab, sessionTabs } = await import("./sessionTabs");
const { answerOf } = await import("./appCalls");
const { useApp } = await import("./store");

const AGENT = "app-call-agent-session";
const USER = "app-call-user-session";

/** The shell and daemon, faked: every command the window was asked for is kept. */
function fakeShell() {
  const commands: string[] = [];
  const answers: { id: string; result: AppAnswer }[] = [];
  mockIPC((command, payload) => {
    commands.push(command);
    const args = payload as { id: string; url: string; events?: Channel<BrowserEvent>; op: Record<string, unknown>; request?: Request };
    switch (command) {
      case "browser_open": {
        const events = args.events!;
        setTimeout(() => events.onmessage({ type: "load", url: args.url, loading: false }), 1);
        return null;
      }
      case "browser_agent":
        return { url: args.url, title: "Page", text: "" };
      case "ipc_request": {
        const sent = args.request as Request;
        if (sent.method === "appAnswer") answers.push({ id: sent.id, result: sent.result });
        return { method: sent.method };
      }
      default:
        return null;
    }
  });
  return { commands, answers };
}

let calls = 0;
const call = (conversationId: string, op: AppOp): AppCall => ({ id: `app-call-${++calls}`, conversationId, taskId: null, label: "The thread", op });

/** Every app call the window runs today, on a session the user isn't looking at. */
const EVERY_CALL: AppOp[] = [
  { app: "browser", op: { op: "open", url: "http://127.0.0.1:4000/" } },
  { app: "state" },
];

test("no app call brings the window forward or moves the user off the session and tab they are on", async () => {
  useApp.setState((state) => ({ settings: { ...state.settings, browserEnabled: true }, selection: { type: "conversation", id: USER } }));
  const shell = fakeShell();
  const theirs = newSessionTab(USER, "terminal", "/tmp");
  for (const op of EVERY_CALL) {
    const sent = call(AGENT, op);
    await runAppCall(sent);
    assert.ok(shell.answers.some((each) => each.id === sent.id), `${op.app} was answered`);
  }
  assert.deepEqual(useApp.getState().selection, { type: "conversation", id: USER }, "the user's session stays in front");
  assert.equal(sessionTabs(USER).active, theirs, "and so does their tab");
  const focusing = shell.commands.filter((command) => /focus|activate|unminimize|set_?show|window\|show/i.test(command));
  assert.deepEqual(focusing, [], "nothing asked the window to come forward");
});

test("the state call tells the session's tabs, whose each is, and whether it is on screen", async () => {
  fakeShell();
  const session = "app-call-state-session";
  useApp.setState({ selection: { type: "conversation", id: session }, windowVisible: true });
  const terminal = newSessionTab(session, "terminal", "/repo");
  const agent = openAgentTab(session, "https://example.com/", { label: "The thread", taskId: null });
  const shown = await answerOf(call(session, { app: "state" }));
  assert.equal(shown.app, "state");
  const view = (shown as Extract<typeof shown, { app: "state" }>).view;
  assert.equal(view.onScreen, true);
  assert.equal(view.front, terminal, "the user's terminal stays in front; the agent's tab opened behind");
  assert.deepEqual(view.tabs.map((tab) => [tab.id, tab.kind, tab.agent, tab.url]), [
    [terminal, "terminal", false, null],
    [agent, "browser", true, "https://example.com/"],
  ]);
  assert.equal(view.tabs[0]!.title, "repo");

  useApp.setState({ selection: { type: "conversation", id: USER } });
  const elsewhere = await answerOf(call(session, { app: "state" }));
  assert.equal((elsewhere as Extract<typeof elsewhere, { app: "state" }>).view.onScreen, false, "the user is on another session");
  useApp.setState({ selection: { type: "conversation", id: session }, windowVisible: false });
  const hidden = await answerOf(call(session, { app: "state" }));
  assert.equal((hidden as Extract<typeof hidden, { app: "state" }>).view.onScreen, false, "the window is hidden");
  useApp.setState({ windowVisible: true });
});
