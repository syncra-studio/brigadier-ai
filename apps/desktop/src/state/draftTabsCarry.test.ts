import assert from "node:assert/strict";
import { beforeEach, test } from "node:test";
import { mockIPC } from "@/ipc/nativeMocks";

import type { Conversation, Message } from "@/ipc/generated";

const data = new Map<string, string>();
Object.defineProperty(globalThis, "localStorage", { configurable: true, value: {
  getItem: (key: string) => data.get(key) ?? null,
  setItem: (key: string, value: string) => data.set(key, value),
  removeItem: (key: string) => data.delete(key),
} });
Object.defineProperty(globalThis, "window", { configurable: true, value: { localStorage, addEventListener: () => {}, removeEventListener: () => {} } });
const { send } = await import("./actions");
const { emptyDraft, useApp } = await import("./store");
const { shellOwner, tabTarget, tabsIdOf, tabsOfTarget } = await import("./draftTabs");
const { carryDraftTabs, CHAT_TAB, closeTab, newSessionTab, openConflictTab, openFileTab, openReviewTab, sessionTabs, useSessionTabs } = await import("./sessionTabs");
await import("./sessionTabResources");

const DRAFT = "draft:project";
const NEW_ID = "made";
const draftSelection = () => ({ type: "draft", kind: "session", projectId: "project" }) as const;
const target = { kind: "session", projectId: "project", setup: null } as const;
const outgoing = { text: "Go", attachments: [], mentions: [] };
const made = { id: NEW_ID, kind: "session", title: "", projectId: "project", setup: null, lifecycle: "active" } as unknown as Conversation;
const sentMessage: Message = { id: "m", conversationId: NEW_ID, seq: 1, role: "user", text: "Go", attachments: [], mentions: [], createdAtMs: 1, blob: null, model: null, requestId: "m", parentId: null };

let methods: string[];
let closedShells: string[];
let daemon: { create: () => Promise<void>; send: () => void };

beforeEach(() => {
  methods = [];
  closedShells = [];
  daemon = { create: async () => {}, send: () => {} };
  useSessionTabs.setState({ sessions: {} });
  useApp.setState({ selection: draftSelection(), draft: emptyDraft("project"), pending: [], conversations: {} });
  mockIPC(async (_command, payload) => {
    const req = (payload as { request: { method: string; terminalId?: string } }).request;
    methods.push(req.method);
    if (req.method === "createConversation") {
      await daemon.create();
      return { method: req.method, conversation: made };
    }
    if (req.method === "sendMessage") {
      daemon.send();
      return { method: req.method, outcome: { type: "sent", message: sentMessage } };
    }
    if (req.method === "closeTerminal") closedShells.push(req.terminalId!);
    return { method: req.method };
  });
});

/** A creation that ends when told. */
function deferred(): { promise: Promise<void>; resolve: () => void } {
  let resolve!: () => void;
  const promise = new Promise<void>((done) => { resolve = done; });
  return { promise, resolve };
}

/** README in a preview tab, the worktree's copy and a Review tab; Review in front. */
function openDraftTabs(): void {
  openFileTab(DRAFT, "README.md", { preview: true });
  openFileTab(DRAFT, "src/main.ts", { checkout: "/wt/one" });
  openReviewTab(DRAFT, { type: "file", path: "server.ts", staged: false });
}

test("a tab from a worktree's row names its checkout: two worktrees' same path are two tabs", () => {
  openFileTab(DRAFT, "src/main.ts", { checkout: "/wt/one" });
  openFileTab(DRAFT, "src/main.ts", { checkout: "/wt/two" });
  openFileTab(DRAFT, "src/main.ts");
  openReviewTab(DRAFT, { type: "changes", staged: false }, undefined, "/wt/one");
  openReviewTab(DRAFT, { type: "changes", staged: false }, undefined, "/wt/two");
  openConflictTab(DRAFT, "a.ts", "conflict", "/wt/one");
  openConflictTab(DRAFT, "a.ts", "conflict", "/wt/two");
  const { tabs } = sessionTabs(DRAFT);
  assert.equal(new Set(tabs.map((tab) => tab.id)).size, 7);
  assert.deepEqual(tabs.flatMap((tab) => (tab.kind === "file" ? [tab.checkout ?? null] : [])), ["/wt/one", "/wt/two", null]);
  // Opened again, each comes to the front rather than adding one.
  openFileTab(DRAFT, "src/main.ts", { checkout: "/wt/one" });
  assert.equal(sessionTabs(DRAFT).tabs.length, 7);
  assert.equal(sessionTabs(DRAFT).active, tabs[0]!.id);
});

test("each owner and row reads its own target", () => {
  assert.deepEqual(tabTarget("c1"), { type: "conversation", conversationId: "c1" });
  assert.deepEqual(tabTarget(DRAFT), { type: "project", projectId: "project" });
  assert.deepEqual(tabTarget(DRAFT, "/wt/one"), { type: "worktree", projectId: "project", path: "/wt/one" });
  assert.deepEqual(tabsOfTarget({ type: "worktree", projectId: "project", path: "/wt/one" }), { owner: DRAFT, checkout: "/wt/one" });
  assert.deepEqual(tabsOfTarget({ type: "project", projectId: "project" }), { owner: DRAFT, checkout: undefined });
  assert.deepEqual(tabsOfTarget({ type: "conversation", conversationId: "c1" }), { owner: "c1", checkout: undefined });
  assert.equal(tabsIdOf(draftSelection()), DRAFT);
  assert.equal(tabsIdOf({ type: "draft", kind: "chat" }), "draft:none");
  assert.equal(tabsIdOf({ type: "none" }), null);
});

test("creation fails: the tabs stay with the draft, as they were", async () => {
  openDraftTabs();
  const before = sessionTabs(DRAFT);
  daemon.create = async () => { throw { kind: "invalid", message: "no" }; };
  await assert.rejects(send(outgoing, target));
  assert.equal(sessionTabs(DRAFT), before);
  assert.equal(useSessionTabs.getState().sessions[NEW_ID], undefined);
  assert.deepEqual(useApp.getState().selection, draftSelection());
});

test("creation succeeds: the tabs follow the session's copy, in order, with the one in front", async () => {
  openDraftTabs();
  const [first, second, third] = sessionTabs(DRAFT).tabs;
  await send(outgoing, target);
  assert.equal(useSessionTabs.getState().sessions[DRAFT], undefined);
  const { tabs, active } = sessionTabs(NEW_ID);
  assert.deepEqual(tabs.map((tab) => tab.id), ["file:README.md", "file:src/main.ts", "review"]);
  assert.equal(active, "review");
  assert.equal(tabs.some((tab) => "checkout" in tab), false, "no tab keeps a worktree");
  assert.deepEqual(tabs.map((tab) => tab.kind === "file" && tab.carried), [true, true, false]);
  assert.equal(tabs[0]!.kind === "file" && tabs[0]!.preview, true);
  // Whoever opened whom still holds, under their new ids.
  assert.equal(first!.opener, CHAT_TAB);
  assert.equal(second!.opener, first!.id);
  assert.equal(tabs[1]!.opener, "file:README.md");
  assert.equal(third!.opener, second!.id);
  assert.equal(tabs[2]!.opener, "file:src/main.ts");
  assert.deepEqual(useApp.getState().selection, { type: "conversation", id: NEW_ID });
});

test("creation succeeds, then the send fails: the tabs are the conversation's", async () => {
  openDraftTabs();
  daemon.send = () => { throw { kind: "invalid", message: "refused" }; };
  await assert.rejects(send(outgoing, target));
  assert.deepEqual(sessionTabs(NEW_ID).tabs.map((tab) => tab.id), ["file:README.md", "file:src/main.ts", "review"]);
  assert.equal(useSessionTabs.getState().sessions[DRAFT], undefined);
  assert.deepEqual(useApp.getState().selection, { type: "conversation", id: NEW_ID });
});

test("navigating while it is made: the tabs go with the draft that started it, and the view stays", async () => {
  openDraftTabs();
  const other = "draft:other";
  openFileTab(other, "theirs.ts");
  const theirs = sessionTabs(other);
  const creating = deferred();
  daemon.create = () => creating.promise;
  const sending = send(outgoing, target);
  const elsewhere = { type: "draft", kind: "session", projectId: "other" } as const;
  useApp.setState({ selection: elsewhere });
  creating.resolve();
  await sending;
  assert.equal(useApp.getState().selection, elsewhere);
  assert.deepEqual(sessionTabs(NEW_ID).tabs.map((tab) => tab.id), ["file:README.md", "file:src/main.ts", "review"]);
  assert.equal(useSessionTabs.getState().sessions[DRAFT], undefined);
  assert.equal(sessionTabs(other), theirs);
});

test("a draft revisited while it is made keeps what was opened since; a later draft starts its own", async () => {
  openDraftTabs();
  const creating = deferred();
  daemon.create = () => creating.promise;
  const sending = send(outgoing, target);
  // Left and came back: a selection of its own, its own new tab.
  useApp.setState({ selection: { type: "none" } });
  const back = draftSelection();
  useApp.setState({ selection: back });
  openFileTab(DRAFT, "later.ts");
  const later = sessionTabs(DRAFT).active;
  creating.resolve();
  await sending;
  assert.equal(useApp.getState().selection, back);
  assert.deepEqual(sessionTabs(NEW_ID).tabs.map((tab) => tab.id), ["file:README.md", "file:src/main.ts", "review"]);
  assert.deepEqual(sessionTabs(DRAFT).tabs.map((tab) => tab.id), [later]);
  assert.equal(sessionTabs(DRAFT).active, later);
  // Sent while still looking at it, the same draft leaves nothing behind, and a new one is empty.
  useSessionTabs.setState({ sessions: {} });
  useApp.setState({ selection: draftSelection() });
  daemon.create = async () => {};
  await send(outgoing, target);
  assert.deepEqual(sessionTabs(DRAFT).tabs, []);
  useApp.setState({ selection: draftSelection() });
  openFileTab(DRAFT, "fresh.ts");
  assert.deepEqual(sessionTabs(DRAFT).tabs.map((tab) => tab.id), ["file:fresh.ts"]);
});

test("two worktrees' same file become one tab, the one in front staying in front", () => {
  openFileTab(DRAFT, "src/main.ts", { checkout: "/wt/one" });
  const first = sessionTabs(DRAFT).active;
  openFileTab(DRAFT, "other.ts");
  openFileTab(DRAFT, "src/main.ts", { checkout: "/wt/two" });
  carryDraftTabs(DRAFT, NEW_ID);
  assert.ok(first);
  assert.deepEqual(sessionTabs(NEW_ID).tabs.map((tab) => tab.id), ["file:src/main.ts", "file:other.ts"]);
  assert.equal(sessionTabs(NEW_ID).active, "file:src/main.ts");
});

test("a terminal tab keeps naming the shell it started with, which goes on running", async () => {
  const id = newSessionTab(DRAFT, "terminal", null);
  const draftTab = sessionTabs(DRAFT).tabs[0]!;
  assert.equal(draftTab.kind, "terminal");
  if (draftTab.kind !== "terminal") return;
  assert.deepEqual(shellOwner(DRAFT, draftTab), { projectId: "project" });
  assert.deepEqual(shellOwner("draft:none", draftTab), {});
  const { openMainTerminal } = await import("./sessionTabResources");
  const asked: unknown[] = [];
  mockIPC((_command, payload) => {
    const req = (payload as { request: { method: string; terminalId?: string } }).request;
    if (req.method === "closeTerminal") closedShells.push(req.terminalId!);
    asked.push(req);
    return { method: req.method, terminal: { id: "shell-1", cwd: "/project" } };
  });
  await openMainTerminal(DRAFT, id, 80, 24);
  carryDraftTabs(DRAFT, NEW_ID);
  await new Promise((resolve) => setTimeout(resolve));
  assert.deepEqual(closedShells, [], "the move isn't a close");
  const carried = sessionTabs(NEW_ID).tabs[0]!;
  assert.equal(carried.id, id);
  assert.equal(carried.kind === "terminal" && carried.homeShell?.projectId, "project");
  await openMainTerminal(NEW_ID, id, 80, 24);
  const attach = asked.at(-1) as { method: string; projectId?: string; conversationId?: string; fresh?: boolean };
  assert.equal(attach.method, "openTerminal");
  assert.equal(attach.projectId, "project");
  assert.equal(attach.conversationId, undefined);
  assert.equal(attach.fresh, undefined);
  // Closing it in the session does end it.
  closeTab(NEW_ID, id);
  await new Promise((resolve) => setTimeout(resolve));
  assert.deepEqual(closedShells, ["shell-1"]);
});


test("a terminal whose open finishes after carry-over remains open", async () => {
  const id = newSessionTab(DRAFT, "terminal", null);
  const ready = deferred();
  mockIPC(async (_command, payload) => {
    const req = (payload as { request: { method: string; terminalId?: string } }).request;
    if (req.method === "closeTerminal") closedShells.push(req.terminalId!);
    if (req.method === "openTerminal") await ready.promise;
    return { method: req.method, terminal: { id: "late-shell", cwd: "/project" } };
  });
  const { openMainTerminal } = await import("./sessionTabResources");
  const opening = openMainTerminal(DRAFT, id, 80, 24);
  carryDraftTabs(DRAFT, NEW_ID);
  ready.resolve();
  await opening;
  assert.deepEqual(closedShells, []);
  const tab = sessionTabs(NEW_ID).tabs.find((each) => each.id === id)!;
  assert.equal(tab.kind === "terminal" && tab.cwd, "/project");
  closeTab(NEW_ID, id);
  await new Promise((resolve) => setTimeout(resolve));
  assert.deepEqual(closedShells, ["late-shell"]);
});

test("colliding Review tabs carry the active view at the first tab's position", () => {
  openReviewTab(DRAFT, { type: "file", path: "first.ts", staged: false }, undefined, "/wt/one");
  openReviewTab(DRAFT, { type: "file", path: "active.ts", staged: true }, undefined, "/wt/two");
  carryDraftTabs(DRAFT, NEW_ID);
  const { tabs, active } = sessionTabs(NEW_ID);
  assert.equal(tabs.length, 1);
  assert.equal(active, tabs[0]!.id);
  assert.deepEqual(tabs[0]!.kind === "review" && tabs[0]!.target, { type: "file", path: "active.ts", staged: true });
});


test("a revisited draft keeps a closed and reopened file with the same id", async () => {
  openFileTab(DRAFT, "same.ts");
  const id = sessionTabs(DRAFT).active;
  const creating = deferred();
  daemon.create = () => creating.promise;
  const sending = send(outgoing, target);
  useApp.setState({ selection: { type: "draft", kind: "chat" } });
  useApp.setState({ selection: draftSelection() });
  closeTab(DRAFT, id);
  openFileTab(DRAFT, "same.ts", { line: 99 });
  const revisited = sessionTabs(DRAFT);
  creating.resolve();
  await sending;
  assert.equal(sessionTabs(DRAFT), revisited);
  assert.equal(sessionTabs(NEW_ID).tabs.length, 0);
});

test("a chat draft carries its page and terminal into a plain chat", async () => {
  useApp.setState({ selection: { type: "draft", kind: "chat" } });
  const owner = "draft:none";
  newSessionTab(owner, "page", null);
  newSessionTab(owner, "terminal", null);
  const before = sessionTabs(owner);
  mockIPC((_command, payload) => {
    const req = (payload as { request: { method: string; kind?: string; projectId?: string | null } }).request;
    if (req.method === "createConversation") {
      assert.equal(req.kind, "chat");
      assert.equal(req.projectId, null);
      return { method: req.method, conversation: { ...made, kind: "chat", projectId: null } };
    }
    if (req.method === "sendMessage") return { method: req.method, outcome: { type: "sent", message: sentMessage } };
    return { method: req.method };
  });
  await send(outgoing, { kind: "chat", setup: null });
  const after = sessionTabs(NEW_ID);
  assert.deepEqual(after.tabs.map((tab) => tab.id), before.tabs.map((tab) => tab.id));
  assert.equal(after.active, before.active);
  assert.equal(sessionTabs(owner).tabs.length, 0);
  const terminal = after.tabs.find((tab) => tab.kind === "terminal")!;
  assert.equal(terminal.kind, "terminal");
  if (terminal.kind === "terminal") assert.deepEqual(shellOwner(NEW_ID, terminal), {});
});
