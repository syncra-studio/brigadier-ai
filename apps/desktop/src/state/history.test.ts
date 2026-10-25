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
const providers: Selection = { type: "settings", page: "providers" };
const claude: Selection = { type: "settings", page: "providers", detail: { provider: "claude" } };
const codex: Selection = { type: "settings", page: "providers", detail: { provider: "codex" } };
const shown = () => useApp.getState().selection;
const places = () => useHistory.getState().entries;
const conversation = (id: string) => ({ id, kind: "chat", title: id, sideOf: null }) as Conversation;
const message = (id: string): Message => ({
  id: `${id}-m1`, conversationId: id, seq: 1, role: "user", text: `message in ${id}`, attachments: [], mentions: [],
  createdAtMs: 1, blob: null, model: null, requestId: null, parentId: null,
});

beforeEach(() => {
  frames.length = 0;
  forgetKeptBoards();
  useBoard.setState({ board: null });
  useHistory.setState({ entries: [], index: -1 });
  useAgentPageAsk.setState({ ask: null });
  useReveal.setState({ label: null });
  useApp.setState((state) => ({ selection: home, conversations: {}, projects: {}, threads: {}, providers: { ...state.providers, view: null } }));
  mockIPC((_command, payload) => {
    const req = (payload as { request: { method: string; id: string } }).request;
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

test("provider list/detail round trips survive other Settings pages and different providers", () => {
  openSettings("general");
  openSettings("providers");
  openAgentPage("claude");
  openAgentPage("codex");
  openSettings("git");
  assert.deepEqual(places(), [home, { type: "settings", page: "general" }, providers, claude, codex, { type: "settings", page: "git" }]);
  for (const target of places().slice(0, -1).toReversed()) {
    stepHistory(-1);
    assert.deepEqual(shown(), target);
  }
  stepHistory(-1);
  assert.deepEqual(shown(), home);
  for (const target of places().slice(1)) {
    stepHistory(1);
    assert.deepEqual(shown(), target);
  }
  stepHistory(1);
  assert.equal(useHistory.getState().index, 5);
});

test("new navigation after Back drops Forward while revisiting the same place adds no loop", () => {
  openSettings("providers");
  openAgentPage("claude");
  openAgentPage("claude");
  assert.deepEqual(places(), [home, providers, claude]);
  stepHistory(-1);
  openSettings("voice");
  assert.deepEqual(places(), [home, providers, { type: "settings", page: "voice" }]);
  stepHistory(1);
  assert.deepEqual(shown(), { type: "settings", page: "voice" });
});

test("Browser nested pages and plugin details are restored as app destinations", () => {
  openSettings("browser");
  for (const view of ["history", "downloads", "sites"] as const) {
    showBrowserSettingsView(view);
    const destination = shown();
    stepHistory(-1);
    stepHistory(1);
    assert.deepEqual(shown(), destination);
  }
  openSettings("plugins");
  select({ type: "settings", page: "plugins", detail: { plugin: "server:example" } });
  openSettings("general");
  stepHistory(-1);
  assert.deepEqual(shown(), { type: "settings", page: "plugins", detail: { plugin: "server:example" } });
  stepHistory(-1);
  assert.deepEqual(shown(), { type: "settings", page: "plugins" });
});

test("external Browser links open one destination without a phantom parent entry", () => {
  openBrowserSettings("downloads");
  assert.deepEqual(places(), [home, { type: "settings", page: "browser", detail: { view: "downloads" } }]);
  stepHistory(-1);
  assert.deepEqual(shown(), home);
});

test("provider sign-in is visit state, canceled by Back and absent on Forward", () => {
  openSettings("providers");
  openAgentPage("codex", true);
  assert.equal(useAgentPageAsk.getState().ask?.under, shown());
  assert.deepEqual(places(), [home, providers, codex]);
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
  assert.deepEqual(places(), [home, providers, claude]);
  assert.equal(useReveal.getState().label, "Accounts");
  const pending = frames.splice(0);
  stepHistory(-1);
  assert.equal(useReveal.getState().label, null);
  // A stale callback returns before asking the DOM for a row (there is no DOM here).
  for (const callback of pending) callback(0);
  assert.deepEqual(shown(), providers);
  assert.deepEqual(places(), [home, providers, claude]);
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
  assert.deepEqual(shown(), claude);
});

test("close and toggle still leave all of Settings for its entry destination", () => {
  useApp.setState({ projects: { p: { id: "p" } as Project } });
  select({ type: "draft", kind: "session", projectId: "p" });
  openSettings("providers");
  openAgentPage("claude");
  openSettings("git");
  closeSettings();
  assert.deepEqual(shown(), { type: "draft", kind: "session", projectId: "p" });
  stepHistory(-1);
  toggleSettings();
  assert.deepEqual(shown(), { type: "draft", kind: "session", projectId: "p" });
});

test("conversation Back/Forward keeps loaded messages and board, and replaces deleted targets", async () => {
  useApp.setState({ conversations: { a: conversation("a"), b: conversation("b") } });
  await select({ type: "conversation", id: "a" });
  const board = useBoard.getState().board;
  const thread = useApp.getState().threads.a;
  await select({ type: "conversation", id: "b" });
  openSettings("providers");
  openAgentPage("claude");
  stepHistory(-1);
  stepHistory(-1);
  assert.deepEqual(shown(), { type: "conversation", id: "b" });
  stepHistory(-1);
  assert.deepEqual(shown(), { type: "conversation", id: "a" });
  assert.equal(useBoard.getState().board, board);
  assert.equal(useApp.getState().threads.a?.items, thread?.items);
  assert.deepEqual(thread?.items.map((item) => item.text), ["message in a"]);
  stepHistory(1);
  assert.deepEqual(shown(), { type: "conversation", id: "b" });
  useApp.setState({ conversations: { b: conversation("b") } });
  stepHistory(-1);
  assert.deepEqual(shown(), home);
  assert.deepEqual(places()[1], home);
});
