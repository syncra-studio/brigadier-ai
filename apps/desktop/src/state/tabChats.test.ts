import assert from "node:assert/strict";
import { beforeEach, test } from "node:test";
import { mockIPC } from "@tauri-apps/api/mocks";

import type { RequestOf } from "@/ipc/client";

const data = new Map<string, string>();
Object.defineProperty(globalThis, "localStorage", { configurable: true, value: {
  getItem: (key: string) => data.get(key) ?? null,
  setItem: (key: string, value: string) => data.set(key, value),
  removeItem: (key: string) => data.delete(key),
} });
Object.defineProperty(globalThis, "window", { configurable: true, value: { localStorage } });
const { closeTab, newSessionTab, openBrowserTab, openFileTab, openPageTab, reopenTab, restoreSessionTabs, sessionTabs } = await import("./sessionTabs");
// Its subscriptions are how a tab's chat goes with the tab.
await import("./sessionTabResources");
const { canChat, chatTarget, keepTabChats, newTabChat, setTabChatShare, setTabChatView, showTabChat, MAX_SHARE, MIN_SHARE } = await import("./tabChats");
const { abandonedSideChats } = await import("./sideChats");

type Sent = RequestOf<"delete"> | RequestOf<"keepTabChats">;
let sent: Sent[];

beforeEach(() => {
  sent = [];
  mockIPC((_command, payload) => {
    const req = (payload as { request: Sent }).request;
    sent.push(req);
    return { method: req.method };
  });
});

const chatOf = (session: string, tabId: string) => {
  const tab = sessionTabs(session).tabs.find((entry) => entry.id === tabId);
  return tab && canChat(tab) ? tab.chat : undefined;
};
const deleted = () => sent.flatMap((req) => req.method === "delete" ? req.ids : []);
const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

test("page, file and browser tabs can chat; terminal, New tab and Review can't, nor a page not made yet", () => {
  const session = "kinds";
  const page = openPageTab(session, "p1", "Notes");
  openFileTab(session, "src/a.ts");
  const browser = openBrowserTab(session, "https://example.com");
  const terminal = newSessionTab(session, "terminal");
  const blank = newSessionTab(session, "newTab");
  const tabs = sessionTabs(session).tabs;
  const target = (id: string) => { const tab = tabs.find((entry) => entry.id === id)!; return canChat(tab) ? chatTarget(tab) : "none"; };
  assert.deepEqual(target(page), { type: "page", pageId: "p1" });
  assert.deepEqual(target("file:src/a.ts"), { type: "file", path: "src/a.ts" });
  assert.deepEqual(target(browser), { type: "browser" });
  assert.equal(target(terminal), "none");
  assert.equal(target(blank), "none");
  assert.equal(canChat({ kind: "review", id: "review", opener: null, target: { type: "all" } } as never), false);
  assert.equal(chatTarget({ kind: "page", id: "draft", opener: null, pageId: null, title: "" }), null);
});

test("the chat opens as a half split, folds and hides keeping its id and width, and comes back as it was", () => {
  const session = "views";
  const page = openPageTab(session, "p2", "Plan");
  showTabChat(session, page);
  const first = chatOf(session, page)!;
  assert.equal(first.view, "split");
  assert.equal(first.share, 0.5);
  setTabChatShare(session, page, 0.62);
  setTabChatView(session, page, "minimized", 100);
  assert.deepEqual(chatOf(session, page), { id: first.id, view: "minimized", share: 0.62, seenAt: 100 });
  showTabChat(session, page);
  setTabChatView(session, page, "hidden", 200);
  assert.deepEqual(chatOf(session, page), { id: first.id, view: "hidden", share: 0.62, seenAt: 200 });
  showTabChat(session, page);
  assert.deepEqual(chatOf(session, page), { id: first.id, view: "split", share: 0.62, seenAt: 200 });
  // The divider goes no further than a quarter either way.
  setTabChatShare(session, page, 0.95);
  assert.equal(chatOf(session, page)!.share, MAX_SHARE);
  setTabChatShare(session, page, 0.01);
  assert.equal(chatOf(session, page)!.share, MIN_SHARE);
  assert.deepEqual(deleted(), []);
});

test("the split and its width survive a restart with the tab", () => {
  const session = "restart";
  const page = openPageTab(session, "p3", "Kept");
  showTabChat(session, page);
  setTabChatShare(session, page, 0.4);
  const chat = chatOf(session, page)!;
  const saved = JSON.parse(data.get("brigadier.sessionTabs")!).state.sessions as Parameters<typeof restoreSessionTabs>[0];
  const restored = restoreSessionTabs(saved)[session]!.tabs.find((tab) => tab.id === page)!;
  assert.deepEqual(canChat(restored) && restored.chat, chat);
});

test("New chat starts over as wide in the same half, and the old chat is deleted", async () => {
  const session = "fresh";
  const page = openPageTab(session, "p4", "Fresh");
  showTabChat(session, page);
  setTabChatShare(session, page, 0.3);
  const old = chatOf(session, page)!.id;
  newTabChat(session, page);
  const now = chatOf(session, page)!;
  assert.notEqual(now.id, old);
  assert.deepEqual({ view: now.view, share: now.share }, { view: "split", share: 0.3 });
  await settle();
  assert.deepEqual(deleted(), [old]);
});

test("closing a tab deletes its chat at once, and the tab reopened starts with none", async () => {
  const session = "close";
  const page = openPageTab(session, "p5", "Gone");
  showTabChat(session, page);
  const id = chatOf(session, page)!.id;
  closeTab(session, page);
  await settle();
  assert.deepEqual(deleted(), [id]);
  assert.equal(reopenTab(session), true);
  const reopened = sessionTabs(session).tabs.find((tab) => tab.kind === "page" && tab.pageId === "p5")!;
  assert.equal(canChat(reopened) && reopened.chat, undefined);
  assert.deepEqual(deleted(), [id]);
});

test("a preview file tab replaced by another takes its chat with it", async () => {
  const session = "preview";
  openFileTab(session, "src/one.ts", { preview: true });
  showTabChat(session, "file:src/one.ts");
  const id = chatOf(session, "file:src/one.ts")!.id;
  openFileTab(session, "src/two.ts", { preview: true });
  await settle();
  assert.deepEqual(deleted(), [id]);
  assert.equal(chatOf(session, "file:src/two.ts"), undefined);
});

test("after a connect the daemon hears every tab chat the tabs still have", async () => {
  const session = "keep";
  const page = openPageTab(session, "p6", "Keep");
  showTabChat(session, page);
  const browser = openBrowserTab(session, "https://example.org");
  showTabChat(session, browser);
  setTabChatView(session, browser, "hidden", 1);
  await keepTabChats();
  const kept = sent.find((req) => req.method === "keepTabChats");
  assert.ok(kept?.method === "keepTabChats");
  assert.ok(kept.chatIds.includes(chatOf(session, page)!.id));
  assert.ok(kept.chatIds.includes(chatOf(session, browser)!.id));
});

test("tab chats are never pruned as abandoned side chats; the daemon sorts them out", () => {
  const conversations = [
    { id: "session", kind: "session" as const, sideOf: null },
    { id: "side", kind: "chat" as const, sideOf: "session" },
    { id: "tab", kind: "chat" as const, sideOf: "session", tabChat: { tabId: "page:1", target: { type: "browser" as const } } },
  ];
  assert.deepEqual(abandonedSideChats(conversations, new Set()), ["side"]);
});
