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
const { newSessionTab, openAgentTab, openFileTab, openPageTab, useSessionTabs, openReviewTab, reviewScope, selectTab, sessionTabs } = await import("./sessionTabs");
const { useWorkerPanelAsk } = await import("./workerPanel");
const { answerOf } = await import("./appCalls");
const { useApp } = await import("./store");
const { usePageEdits } = await import("./pageEditing");
const { useAgentPause } = await import("./agentPause");

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
  { app: "browser", op: { op: "open", url: "http://127.0.0.1:4000/", keep: false, request: "req-1" } },
  { app: "browser", op: { op: "show", tab: "browser:agent-1" } },
  { app: "state" },
  { app: "open", target: { kind: "file", path: "src/main.rs", line: 3 }, show: true },
  { app: "open", target: { kind: "diff", scope: "lastTurn" }, show: true },
  { app: "open", target: { kind: "progress" }, show: true },
  { app: "open", target: { kind: "output", source: "preview", id: "preview-1", title: "Dev app" }, show: true },
  { app: "open", target: { kind: "url", url: "http://127.0.0.1:5173/" }, show: true },
  { app: "open", target: { kind: "worker", task: "task-1" }, show: true },
  { app: "open", target: { kind: "plan", id: "card-1", outline: false, title: "The plan" }, show: true },
  { app: "open", target: { kind: "page", id: "page-1", title: "Notes" }, show: true },
  { app: "open", target: { kind: "terminal", tab: "terminal:agent-1", cwd: "/repo" }, show: true },
  { app: "endRequest", request: "req-1" },
  // The session the user is on already: showing it moves nothing.
  { app: "showSession", session: USER },
  { app: "closeTab", tab: "terminal:agent-1", confirmed: true },
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

type Opened = Extract<AppAnswer, { app: "opened" }>;
const opened = async (conversationId: string, target: Extract<AppOp, { app: "open" }>["target"], show: boolean) =>
  (await answerOf(call(conversationId, { app: "open", target, show }))) as Opened | Extract<AppAnswer, { app: "failed" }>;

test("open_in_app opens in front only when asked to show and the user is on the session; else behind with its dot", async () => {
  fakeShell();
  const session = "open-in-app-session";
  useApp.setState({ selection: { type: "conversation", id: session }, windowVisible: true });
  const theirs = newSessionTab(session, "terminal", "/repo");

  const behind = await opened(session, { kind: "file", path: "src/a.rs", line: 4 }, false);
  assert.deepEqual(behind, { app: "opened", tab: "file:src/a.rs", front: false, kept: false });
  assert.equal(sessionTabs(session).active, theirs, "not asked to show it: the user's tab stays in front");
  const a = sessionTabs(session).tabs.find((tab) => tab.id === "file:src/a.rs")!;
  assert.equal(a.agentOpened, true);
  assert.equal(a.unseen, true, "it has its dot");

  const shown = await opened(session, { kind: "progress" }, true);
  assert.deepEqual(shown, { app: "opened", tab: "panel:reviews", front: true, kept: false });
  assert.equal(sessionTabs(session).active, "panel:reviews");
  assert.equal(sessionTabs(session).tabs.find((tab) => tab.id === "panel:reviews")!.unseen, undefined, "seen in front");

  selectTab(session, "file:src/a.rs");
  assert.equal(sessionTabs(session).tabs.find((tab) => tab.id === "file:src/a.rs")!.unseen, undefined, "brought to the front, its dot goes");

  useApp.setState({ selection: { type: "conversation", id: USER } });
  selectTab(session, theirs);
  const away = await opened(session, { kind: "output", source: "preview", id: "preview-2", title: "Dev app" }, true);
  assert.deepEqual(away, { app: "opened", tab: "panel:output:preview:preview-2", front: false, kept: false }, "the user is on another session: behind");
  assert.equal(sessionTabs(session).active, theirs);
  assert.deepEqual(useApp.getState().selection, { type: "conversation", id: USER }, "and the user stays where they are");
  useApp.setState({ selection: { type: "conversation", id: session } });
});

test("open_in_app leaves the user's own tab, and the one in front, as they were", async () => {
  fakeShell();
  const session = "open-in-app-user-tabs";
  useApp.setState({ selection: { type: "conversation", id: session }, windowVisible: true });
  openFileTab(session, "src/b.rs", { line: 1 });
  openReviewTab(session, { type: "changes", staged: true });
  const kept = await opened(session, { kind: "file", path: "src/b.rs", line: 40 }, false);
  assert.deepEqual(kept, { app: "opened", tab: "file:src/b.rs", front: false, kept: true });
  const file = sessionTabs(session).tabs.find((tab) => tab.id === "file:src/b.rs")!;
  assert.equal(file.kind === "file" && file.line, 1, "the user's file stays at their line");
  assert.equal(file.agentOpened, undefined, "and stays theirs");
  const review = await opened(session, { kind: "diff", scope: "lastTurn" }, false);
  assert.equal(review.app === "opened" && review.kept, true, "the Review tab in front isn't changed under the user");
  const reviewTab = sessionTabs(session).tabs.find((tab) => tab.kind === "review");
  assert.deepEqual(reviewTab?.kind === "review" && reviewTab.target, { type: "changes", staged: true });

  const asked = await opened(session, { kind: "file", path: "src/b.rs", line: 40 }, true);
  assert.deepEqual(asked, { app: "opened", tab: "file:src/b.rs", front: true, kept: false }, "asked to show it: it comes to the front at the line");
  const shownFile = sessionTabs(session).tabs.find((tab) => tab.id === "file:src/b.rs")!;
  assert.equal(shownFile.kind === "file" && shownFile.line, 40);
  assert.equal(shownFile.agentOpened, undefined, "still the user's");
});

test("open_in_app's diffs read the last turn, the branch or everything uncommitted", async () => {
  fakeShell();
  const session = "open-in-app-diffs";
  useApp.setState({ selection: { type: "conversation", id: session }, windowVisible: true });
  const scopes = [];
  for (const scope of ["lastTurn", "branch", "uncommitted"] as const) {
    await opened(session, { kind: "diff", scope }, true);
    const tab = sessionTabs(session).tabs.find((each) => each.kind === "review")!;
    assert.equal(tab.kind, "review");
    if (tab.kind === "review") scopes.push(reviewScope(tab.target));
  }
  assert.deepEqual(scopes, [{ type: "lastTurn", requestId: null }, { type: "branch" }, { type: "uncommitted" }]);
});

test("open_in_app reuses the Browser tab already on the page, and a worker's panel opens only in front", async () => {
  fakeShell();
  const session = "open-in-app-browser";
  useApp.setState((state) => ({ settings: { ...state.settings, browserEnabled: true }, selection: { type: "conversation", id: session }, windowVisible: true }));
  const first = await opened(session, { kind: "url", url: "http://127.0.0.1:5173/" }, true);
  const again = await opened(session, { kind: "url", url: "http://127.0.0.1:5173" }, true);
  assert.equal(first.app === "opened" && again.app === "opened" && first.tab === again.tab, true, "one tab for one page");
  assert.equal(sessionTabs(session).tabs.filter((tab) => tab.kind === "browser").length, 1);

  const panel = await opened(session, { kind: "worker", task: "task-7" }, true);
  assert.deepEqual(panel, { app: "opened", tab: null, front: true, kept: false });
  assert.deepEqual(useWorkerPanelAsk.getState().ask, { conversationId: session, task: "task-7" });
  useWorkerPanelAsk.setState({ ask: null });
  const notAsked = await opened(session, { kind: "worker", task: "task-7" }, false);
  assert.equal(notAsked.app, "failed");
  assert.equal(useWorkerPanelAsk.getState().ask, null, "not asked to show it: no panel");
});

test("terminal_run's tab opens behind as the agent's, and a terminal of the user's is left as it was", async () => {
  fakeShell();
  const session = "open-in-app-terminal";
  useApp.setState({ selection: { type: "conversation", id: session }, windowVisible: true });
  const theirs = newSessionTab(session, "terminal", "/repo");
  const behind = await opened(session, { kind: "terminal", tab: "terminal:agent-9", cwd: "/repo" }, false);
  assert.deepEqual(behind, { app: "opened", tab: "terminal:agent-9", front: false, kept: false });
  const agent = sessionTabs(session).tabs.find((tab) => tab.id === "terminal:agent-9")!;
  assert.deepEqual([agent.kind, agent.agentOpened, agent.unseen], ["terminal", true, true]);
  assert.equal(sessionTabs(session).active, theirs);
  newSessionTab(session, "page");
  const kept = await opened(session, { kind: "terminal", tab: theirs, cwd: "/repo" }, false);
  assert.equal(kept.app === "opened" && kept.kept, true, "the user's own terminal tab isn't touched");
  const shown = await opened(session, { kind: "terminal", tab: theirs, cwd: "/repo" }, true);
  assert.deepEqual(shown, { app: "opened", tab: theirs, front: true, kept: false }, "asked to watch it: it comes to the front");
  assert.equal(sessionTabs(session).tabs.find((tab) => tab.id === theirs)!.agentOpened, undefined, "and stays theirs");
});

test("show_session moves the selection to that session, as its sidebar row would, and never the window", async () => {
  useApp.setState({ selection: { type: "conversation", id: USER }, windowVisible: true });
  const shell = fakeShell();
  const answer = await answerOf(call(AGENT, { app: "showSession", session: "app-call-api-session" }));
  assert.deepEqual(answer, { app: "sessionShown", visible: true });
  assert.deepEqual(useApp.getState().selection, { type: "conversation", id: "app-call-api-session" });
  const focusing = shell.commands.filter((command) => /focus|activate|unminimize|set_?show|window\|show/i.test(command));
  assert.deepEqual(focusing, [], "nothing asked the window to come forward");

  useApp.setState({ windowVisible: false });
  const hidden = await answerOf(call(AGENT, { app: "showSession", session: USER }));
  assert.deepEqual(hidden, { app: "sessionShown", visible: false }, "a hidden window stays hidden");
});

test("close_tab closes the agent's tab at once; the user's, an unsaved page and a terminal only once confirmed", async () => {
  fakeShell();
  const session = "app-call-close-session";
  useApp.setState({ selection: { type: "conversation", id: session }, windowVisible: true });
  const close = async (tab: string, confirmed = false) => answerOf(call(session, { app: "closeTab", tab, confirmed }));
  const ids = () => sessionTabs(session).tabs.map((tab) => tab.id);

  const agents = openAgentTab(session, "https://example.com/", { label: "The thread", taskId: null });
  assert.deepEqual(await close(agents), { app: "tabClosed", tab: agents });
  assert.ok(!ids().includes(agents));

  const theirs = newSessionTab(session, "browser");
  const asked = await close(theirs);
  assert.deepEqual(asked.app === "closeAsks" && [asked.users, asked.unsaved, asked.kind], [true, false, "browser"]);
  assert.ok(ids().includes(theirs), "the user's tab stays open");

  const paused = openAgentTab(session, "https://example.org/", { label: "The thread", taskId: null });
  useAgentPause.setState({ paused: { [paused]: "user" } });
  const pausedAsk = await close(paused);
  assert.equal(pausedAsk.app === "closeAsks" && pausedAsk.users, true, "a tab the user took over is theirs");
  useAgentPause.setState({ paused: {} });

  const page = openPageTab(session, "page-close-1", "Notes");
  useSessionTabs.setState((state) => ({ sessions: { ...state.sessions, [session]: { ...state.sessions[session]!, tabs: state.sessions[session]!.tabs.map((tab) => (tab.id === page ? { ...tab, agentOpened: true } : tab)) } } }));
  usePageEdits.setState({ pages: { "page-close-1": { status: "ready", content: { title: "Notes", emoji: null, body: "x" }, revision: "r1", dirty: true, conflict: null, error: null, version: 1 } } });
  const pageAsk = await close(page);
  assert.deepEqual(pageAsk.app === "closeAsks" && [pageAsk.users, pageAsk.unsaved, pageAsk.title], [false, true, "Notes"], "the agent's page, with edits not saved yet");

  const terminal = "terminal:agent-close-1";
  await answerOf(call(session, { app: "open", target: { kind: "terminal", tab: terminal, cwd: "/repo" }, show: false }));
  const terminalAsk = await close(terminal);
  assert.deepEqual(terminalAsk.app === "closeAsks" && [terminalAsk.users, terminalAsk.kind], [false, "terminal"], "the daemon tells whether it runs something");

  for (const tab of [theirs, page, terminal]) assert.deepEqual(await close(tab, true), { app: "tabClosed", tab });
  assert.deepEqual(ids(), [paused]);
  assert.equal((await close("chat", true)).app, "failed", "the conversation's own tab never closes");
  assert.deepEqual(await close("browser:gone", true), { app: "failed", message: "There is no tab browser:gone in this session now." });
});
