import assert from "node:assert/strict";
import { beforeEach, test } from "node:test";
import { mockIPC } from "@tauri-apps/api/mocks";

import { model, overview } from "@/fixtures/agents";
import type { Conversation, ConversationView, Message, Project } from "@/ipc/generated";
import type { Selection } from "@/state/store";

const data = new Map<string, string>();
Object.defineProperty(globalThis, "localStorage", { configurable: true, value: {
  getItem: (key: string) => data.get(key) ?? null,
  setItem: (key: string, value: string) => data.set(key, value),
  removeItem: (key: string) => data.delete(key),
} });
Object.defineProperty(globalThis, "window", { configurable: true, value: { localStorage, setTimeout, clearTimeout } });
const frames: FrameRequestCallback[] = [];
Object.defineProperty(globalThis, "requestAnimationFrame", { configurable: true, value: (callback: FrameRequestCallback) => frames.push(callback) });
const { openSettings, closeSettings, select, stepHistory, toggleSettings } = await import("./actions");
const { openAgentPage, useAgentPageAsk } = await import("./agentPage");
const { openBrowserSettings, showBrowserSettingsView } = await import("./browserSettings");
const { forgetKeptBoards, useBoard } = await import("./board");
const { useHistory } = await import("./history");
const { useApp } = await import("./store");
const { useReveal, revealSetting } = await import("@/app/settings/reveal");

const home: Selection = { type: "draft", kind: "chat" };
const general: Selection = { type: "settings", page: "general" };
const routing: Selection = { type: "settings", page: "routing" };
const providers: Selection = { type: "settings", page: "providers" };
const claude: Selection = { type: "settings", page: "providers", detail: { provider: "claude" } };
const codex: Selection = { type: "settings", page: "providers", detail: { provider: "codex" } };
const chat = (id: string): Selection => ({ type: "conversation", id });
const shown = () => useApp.getState().selection;
const redo = () => useHistory.getState().forward;
const conversation = (id: string) => ({ id, kind: "chat", title: id, sideOf: null }) as Conversation;
const message = (id: string): Message => ({
  id: `${id}-m1`, conversationId: id, seq: 1, role: "user", text: `message in ${id}`, attachments: [], mentions: [],
  createdAtMs: 1, blob: null, model: null, requestId: null, parentId: null,
});

/** Steps `delta` once per place, each time expecting to arrive at that place. */
function walk(delta: -1 | 1, places: Selection[]): void {
  for (const place of places) {
    stepHistory(delta);
    assert.deepEqual(shown(), place);
  }
}

beforeEach(() => {
  frames.length = 0;
  forgetKeptBoards();
  useBoard.setState({ board: null });
  useHistory.setState({ forward: [] });
  useAgentPageAsk.setState({ ask: null });
  useReveal.setState({ label: null });
  useApp.setState((state) => ({ selection: home, conversations: {}, projects: {}, threads: {}, providers: { ...state.providers, view: null } }));
  mockIPC((_command, payload) => {
    const req = (payload as { request: { method: string; id: string } }).request;
    if (req.method === "threadView") return { method: req.method, view: { entries: [], turns: [] } };
    assert.equal(req.method, "getConversation");
    const view = {
      conversation: conversation(req.id), context: null, messages: { messages: [message(req.id)], hasMore: false },
      tasks: [], approvals: [], questions: [], requests: [], queue: { items: [], paused: false }, run: "idle",
      runRequest: null, head: null, workerSteps: [], orchestratorSteps: [], machineSteps: [], thinking: [], compactions: [],
      decisions: [], ratings: {}, streaming: null, notices: [],
    } as unknown as ConversationView;
    return { method: req.method, view };
  });
});

test("Home, Settings, Routing, Providers: Back skips Routing and ends at Home, Forward redoes those steps", () => {
  openSettings();
  assert.deepEqual(shown(), general);
  openSettings("routing");
  assert.deepEqual(shown(), routing);
  openSettings("providers");
  walk(-1, [general, home, home]);
  assert.deepEqual(redo(), [providers, general]);
  walk(1, [general, providers, providers]);
  assert.deepEqual(redo(), []);
});

test("from a chat, Back goes Home whichever chat came before, and Forward returns to it", async () => {
  useApp.setState({ conversations: { a: conversation("a"), b: conversation("b") } });
  await select(chat("a"));
  await select(chat("b"));
  walk(-1, [home, home]);
  walk(1, [chat("b"), chat("b")]);
});

test("a project's draft goes Home too, and a removed project's draft can't be returned to", () => {
  useApp.setState({ projects: { p: { id: "p" } as Project } });
  const draft: Selection = { type: "draft", kind: "session", projectId: "p" };
  select(draft);
  walk(-1, [home]);
  walk(1, [draft]);
  walk(-1, [home]);
  useApp.setState({ projects: {} });
  walk(1, [home]);
  assert.deepEqual(redo(), []);
});

test("from a chat, Back leaves Settings through General for that chat, then Home", async () => {
  useApp.setState({ conversations: { a: conversation("a") } });
  await select(chat("a"));
  openSettings();
  walk(-1, [chat("a"), home, home]);
  walk(1, [chat("a"), general]);
  // Settings re-entered by Forward still leaves for the same chat.
  walk(-1, [chat("a")]);
});

test("Settings opened from a chat since deleted, or from nothing, leaves for Home", async () => {
  useApp.setState({ conversations: { a: conversation("a") } });
  await select(chat("a"));
  openSettings("providers");
  useApp.setState({ conversations: {} });
  walk(-1, [general, home]);

  useApp.setState({ selection: { type: "none" } });
  stepHistory(-1);
  assert.deepEqual(shown(), { type: "none" });
  openSettings();
  walk(-1, [home]);
});

test("a detail goes to its own page, then General, however it was reached", () => {
  openSettings("git");
  openAgentPage("claude");
  openAgentPage("codex");
  walk(-1, [providers, general, home]);
  walk(1, [general, providers, codex]);
});

test("any new navigation clears Forward; showing the same place again does not", () => {
  openSettings("providers");
  openAgentPage("claude");
  stepHistory(-1);
  assert.deepEqual(redo(), [claude]);
  openSettings("providers");
  assert.deepEqual(redo(), [claude]);
  openSettings("voice");
  assert.deepEqual(redo(), []);
  stepHistory(1);
  assert.deepEqual(shown(), { type: "settings", page: "voice" });

  stepHistory(-1);
  closeSettings();
  assert.deepEqual(shown(), home);
  assert.deepEqual(redo(), []);
});

test("Browser nested pages and plugin details go up to their page and are restored by Forward", () => {
  openSettings("browser");
  const browser: Selection = { type: "settings", page: "browser" };
  for (const view of ["history", "downloads", "sites"] as const) {
    showBrowserSettingsView(view);
    const destination = shown();
    walk(-1, [browser]);
    walk(1, [destination]);
    showBrowserSettingsView("page");
  }
  const plugins: Selection = { type: "settings", page: "plugins" };
  const plugin: Selection = { type: "settings", page: "plugins", detail: { plugin: "server:example" } };
  openSettings("plugins");
  select(plugin);
  walk(-1, [plugins, general]);
  walk(1, [plugins, plugin]);
});

test("an external Browser link goes up through its page like any other", () => {
  openBrowserSettings("downloads");
  walk(-1, [{ type: "settings", page: "browser" }, general, home]);
});

test("provider sign-in is visit state, canceled by Back and absent on Forward", () => {
  openSettings("providers");
  openAgentPage("codex", true);
  assert.equal(useAgentPageAsk.getState().ask?.under, shown());
  stepHistory(-1);
  assert.equal(useAgentPageAsk.getState().ask, null);
  assert.deepEqual(shown(), providers);
  stepHistory(1);
  assert.deepEqual(shown(), codex);
  assert.equal(useAgentPageAsk.getState().ask, null);
});

test("merged links and Settings search resolve detail once before reveal, canceled by Back", () => {
  openSettings("providers");
  openSettings("usage");
  assert.deepEqual(shown(), claude);
  assert.equal(useReveal.getState().label, "Accounts");
  const pending = frames.splice(0);
  stepHistory(-1);
  assert.equal(useReveal.getState().label, null);
  // A stale callback returns before asking the DOM for a row (there is no DOM here).
  for (const callback of pending) callback(0);
  assert.deepEqual(shown(), providers);
  openSettings("providers", "Models");
  assert.deepEqual(shown(), claude);
  openSettings("providers", "Agents");
  assert.deepEqual(shown(), providers);
});

test("a canceled reveal cannot clear a newer reveal request", () => {
  openSettings("providers", "Accounts");
  const old = frames.splice(0);
  openSettings("general");
  revealSetting("Theme");
  for (const callback of old) callback(0);
  assert.equal(useReveal.getState().label, "Theme");
});

test("provider reveals keep the current provider, or resolve the model or first installed agent", () => {
  const missing = overview("claude", []);
  missing.status = { ...missing.status!, path: null };
  useApp.setState((state) => ({ providers: { ...state.providers, view: {
    providers: [missing, overview("codex", [model("gpt", "GPT fixture")])], sessions: [], fixtures: [],
  } } }));
  openSettings("providers");
  openSettings("providers", "Accounts");
  assert.deepEqual(shown(), codex);
  openAgentPage("claude");
  openSettings("providers", "Accounts");
  assert.deepEqual(shown(), claude);
  openSettings("providers", "GPT fixture");
  assert.deepEqual(shown(), codex);
  stepHistory(-1);
  assert.deepEqual(shown(), providers);
});

test("close and toggle still leave all of Settings for its entry destination", () => {
  const draft: Selection = { type: "draft", kind: "session", projectId: "p" };
  useApp.setState({ projects: { p: { id: "p" } as Project } });
  select(draft);
  openSettings("providers");
  openAgentPage("claude");
  openSettings("git");
  closeSettings();
  assert.deepEqual(shown(), draft);
  openSettings("providers");
  openSettings("git");
  stepHistory(-1);
  assert.deepEqual(shown(), general);
  toggleSettings();
  assert.deepEqual(shown(), draft);
});

test("conversation Back/Forward keeps loaded messages and board, and a deleted one shows Home", async () => {
  useApp.setState({ conversations: { a: conversation("a"), b: conversation("b") } });
  await select(chat("a"));
  const board = useBoard.getState().board;
  const thread = useApp.getState().threads.a;
  stepHistory(-1);
  assert.deepEqual(shown(), home);
  stepHistory(1);
  assert.deepEqual(shown(), chat("a"));
  assert.equal(useBoard.getState().board, board);
  assert.equal(useApp.getState().threads.a?.items, thread?.items);
  assert.deepEqual(thread?.items.map((item) => item.text), ["message in a"]);
  stepHistory(-1);
  useApp.setState({ conversations: { b: conversation("b") } });
  stepHistory(1);
  assert.deepEqual(shown(), home);
  assert.deepEqual(redo(), []);
});
