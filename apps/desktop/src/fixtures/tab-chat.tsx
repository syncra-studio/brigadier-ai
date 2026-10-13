/**
 * A standalone Vite entry, excluded from the app entry and production build: the whole app
 * (App.tsx) around a project session with a page, a file, a Review, a New tab and a browser tab
 * open, over IPC answered here. Run by TabChatRender.test.ts with `?drive=1`: which tabs show
 * the round chat button and where, the split it opens (its halves, header and chat), dragging
 * and stepping the divider, folding the chat to a strip and back, hiding it and opening it
 * again as it was, the dot while the hidden chat works or has news, and New chat. It writes
 * what it saw to `<pre id="tab-chat-result">`.
 */
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { App } from "@/app/App";
import type { Conversation, Page, PageContent, Project, Request } from "@/ipc/generated";
import { useActivity } from "@/state/activity";
import { emptyBoard, useBoard } from "@/state/board";
import { openBrowserTab, openFileTab, openPageTab, openReviewTab, newSessionTab, selectTab, sessionTabs, useSessionTabs } from "@/state/sessionTabs";
import { emptyThread, useApp } from "@/state/store";
import { canChat } from "@/state/tabChats";

const query = new URLSearchParams(location.search);
document.documentElement.dataset.platform = "macos";

const ID = "tab-chat-session";
const PAGE = "4".repeat(32);
const start = Date.now() - 86_400_000;

const conversation = {
  id: ID, kind: "session", projectId: "tab-chat-project", title: "Plan the launch",
  pinnedAtMs: null, createdAtMs: start, updatedAtMs: start, lifecycle: "active",
  forkedFrom: null, sideOf: null, fallback: null, quotaWait: null,
  setup: {
    type: "session", repo: "/tmp/tab-chat-fixture",
    environment: { type: "newWorktree", branch: "brigadier/tab-chat/session", base: "main" },
    permission: "fullAccess", planMode: false, workersSeeUncommitted: null,
    orchestrator: { provider: "claude", model: "opus", effort: "high" },
  },
} as unknown as Conversation;

useBoard.setState({ board: { ...emptyBoard(ID), loaded: true } });
useApp.setState((state) => ({
  info: { platform: "macos", version: "fixture", processStartMs: 0, smoke: false, firstLaunch: false, budgetTolerance: 1 },
  connection: { status: "connected", daemon: null, reason: null },
  settings: { ...state.settings, onboarded: true, introSeen: true, browserEnabled: true },
  projects: { "tab-chat-project": { id: "tab-chat-project", name: "launch", createdAtMs: start, repos: [{ path: "/tmp/tab-chat-fixture", name: "launch" }], prefs: {}, trust: [] } as unknown as Project },
  conversations: { [ID]: conversation },
  threads: { [ID]: { ...emptyThread } },
  selection: { type: "conversation", id: ID },
}));
useSessionTabs.setState({ sessions: {} });
const PAGE_TAB = openPageTab(ID, PAGE, "Launch plan");
openFileTab(ID, "src/app.ts");
openReviewTab(ID, { type: "all" });
const NEW_TAB = newSessionTab(ID, "newTab");
const BROWSER_TAB = openBrowserTab(ID, "https://example.com");
selectTab(ID, PAGE_TAB);

const page: Page = { id: PAGE, content: { title: "Launch plan", emoji: null, body: "Ship on Monday." } as PageContent, revision: "r1", createdMs: 0, updatedMs: start } as Page;

/** What the daemon was asked, in order. */
const asked: Request[] = [];
const view = (id: string) => ({
  conversation: useApp.getState().conversations[id], messages: { messages: [], hasMore: false },
  tasks: [], approvals: [], questions: [], plans: [], overnight: [], requests: [], workerSteps: [], orchestratorSteps: [],
  machineSteps: [], decisions: [], waiting: [], compactions: [], ratings: {}, queue: emptyBoard(id).queue,
  run: "idle", runRequest: null, head: null, context: null, streaming: null, thinking: [], notices: [], memories: [], reviews: [], previews: [],
});

mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const request = (payload as { request: Request }).request;
  asked.push(request);
  switch (request.method) {
    case "pageRead": return { method: "pageRead", page };
    case "pageList": return { method: "pageList", pages: [{ id: PAGE, title: "Launch plan", emoji: null, updatedMs: start }] };
    case "readFile": return { method: request.method, file: { text: "export const app = 1;\n", size: 22, truncated: false } };
    case "listMessages": return { method: "listMessages", page: { items: [], hasMore: false } };
    case "openTabChat": return { method: request.method, conversation: {
      ...conversation, id: request.chatId, kind: "chat", title: "Chat", sideOf: request.conversationId,
      tabChat: { tabId: request.tabId, target: request.target },
      setup: { type: "chat", model: { provider: "claude", model: "opus", effort: "high" } },
    } };
    case "getConversation": return { method: request.method, view: view(request.id) };
    default: return { method: request.method };
  }
}, { shouldMockEvents: true });

mockWindows("main");

const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const until = async (check: () => boolean, ms = 5000) => {
  for (const end = Date.now() + ms; Date.now() < end && !check();) await wait(25);
  return check();
};
/** The tab in front's panel. */
const front = () => document.querySelector<HTMLElement>('[role="tabpanel"]:not([inert])');
const inFront = <T extends Element = HTMLElement>(selector: string) => front()?.querySelector<T>(selector) ?? null;
const rect = (element: Element | null) => {
  const box = element?.getBoundingClientRect();
  return box ? { x: Math.round(box.x), y: Math.round(box.y), w: Math.round(box.width), h: Math.round(box.height) } : null;
};
const chatOf = (tabId: string) => {
  const tab = sessionTabs(ID).tabs.find((entry) => entry.id === tabId);
  return tab && canChat(tab) ? tab.chat : undefined;
};
/** The round button in the tab in front: what it says, and whether it floats in the tab's corner. */
const button = () => {
  const slot = inFront('[data-slot="tab-chat-button"]');
  if (!slot) return null;
  const panel = rect(front());
  const at = rect(slot.querySelector("button"));
  return {
    label: slot.querySelector(".sr-only")?.textContent ?? null,
    floating: getComputedStyle(slot).position === "absolute",
    fromTop: panel && at ? at.y - panel.y : null,
    fromEnd: panel && at ? panel.x + panel.w - (at.x + at.w) : null,
    fromBottom: panel && at ? panel.y + panel.h - (at.y + at.h) : null,
    round: at ? getComputedStyle(slot.querySelector("button")!).borderRadius : null,
    dot: slot.querySelector('[data-slot="tab-chat-dot"]') !== null,
  };
};
const click = (element: Element | null) => (element as HTMLElement | null)?.click();
const headerButtons = () => [...(inFront('[data-slot="tab-chat-header"]')?.querySelectorAll(".sr-only") ?? [])].map((label) => label.textContent);
const halves = () => {
  const content = rect(inFront('[data-slot="tab-content"]'));
  const chat = rect(inFront('[data-slot="tab-chat"]'));
  const whole = rect(inFront('[data-slot="tab-with-chat"]'));
  return { content: content?.w ?? 0, chat: chat?.w ?? 0, whole: whole?.w ?? 0, chatShare: whole && chat ? Math.round((chat.w / whole.w) * 100) : 0 };
};
const pointer = (target: Element, type: string, x: number) => {
  const box = target.getBoundingClientRect();
  target.dispatchEvent(new PointerEvent(type, { bubbles: true, clientX: x, clientY: box.top + box.height / 2, button: 0, pointerId: 1 }));
};

async function drive(): Promise<Record<string, unknown>> {
  const seen: Record<string, unknown> = {};
  // Which tabs have the button, and where.
  const buttons: Record<string, unknown> = {};
  for (const [kind, id] of [["page", PAGE_TAB], ["file", "file:src/app.ts"], ["review", "review"], ["newTab", NEW_TAB], ["browser", BROWSER_TAB]] as const) {
    selectTab(ID, id);
    await until(() => front()?.getAttribute("aria-label") !== null && (kind === "review" || kind === "newTab" || button() !== null), 3000);
    await wait(150);
    buttons[kind] = button();
  }
  seen.buttons = buttons;
  seen.reviewTabId = sessionTabs(ID).tabs.find((tab) => tab.kind === "review")?.id;

  // The page's button opens the split, half and half, the chat at the end.
  selectTab(ID, PAGE_TAB);
  await until(() => button() !== null);
  click(inFront('[data-slot="tab-chat-button"] button'));
  await until(() => inFront('[data-slot="tab-chat"]') !== null);
  await until(() => inFront('[data-slot="tab-chat"] [data-embedded-view]') !== null || inFront('[data-slot="tab-chat"] [role="alert"]') !== null);
  await wait(200);
  const chat = chatOf(PAGE_TAB)!;
  seen.opened = {
    halves: halves(), buttonGone: button() === null, header: headerButtons(),
    chatAtEnd: (rect(inFront('[data-slot="tab-chat"]'))?.x ?? 0) > (rect(inFront('[data-slot="tab-content"]'))?.x ?? 0),
    asked: asked.filter((request) => request.method === "openTabChat").map((request) => request.method === "openTabChat" && { tabId: request.tabId, target: request.target, chatId: request.chatId === chat.id }),
    welcome: inFront('[data-slot="tab-chat"] [data-embedded-view]')?.textContent?.includes("Ask about this page") ?? false,
    error: inFront('[data-slot="tab-chat"] [role="alert"]')?.textContent ?? null,
    modelPicker: [...(inFront('[data-slot="tab-chat"]')?.querySelectorAll("button") ?? [])].some((entry) => /opus/i.test(entry.textContent ?? "")),
  };

  // Dragging the divider shares the width differently; it stops at a quarter.
  const divider = inFront('[data-slot="tab-chat-divider"]')!;
  const whole = inFront('[data-slot="tab-with-chat"]')!.getBoundingClientRect();
  pointer(divider, "pointerdown", whole.left + whole.width / 2);
  pointer(divider, "pointermove", whole.left + whole.width * 0.4);
  pointer(divider, "pointerup", whole.left + whole.width * 0.4);
  await wait(100);
  seen.dragged = { share: chatOf(PAGE_TAB)!.share, halves: halves() };
  pointer(divider, "pointerdown", whole.left + whole.width * 0.4);
  pointer(divider, "pointermove", whole.left + 10);
  pointer(divider, "pointerup", whole.left + 10);
  await wait(100);
  seen.draggedFar = chatOf(PAGE_TAB)!.share;
  // The arrow keys step it.
  divider.focus();
  divider.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true, cancelable: true }));
  await wait(50);
  seen.stepped = { share: chatOf(PAGE_TAB)!.share, value: divider.getAttribute("aria-valuenow") };

  // Minimize folds it to a strip at the tab's edge; the strip brings it back.
  click(inFront('[data-slot="tab-chat-header"] button:nth-of-type(2)'));
  await until(() => inFront('[data-slot="tab-chat-strip"]') !== null);
  const strip = inFront('[data-slot="tab-chat-strip"]');
  const panel = rect(front());
  seen.minimized = {
    view: chatOf(PAGE_TAB)!.view, strip: rect(strip), atEnd: panel && strip ? panel.x + panel.w - (rect(strip)!.x + rect(strip)!.w) : null,
    label: strip?.getAttribute("aria-label"), chatGone: inFront('[data-slot="tab-chat"]') === null, button: button(),
    pageFull: halves().content === halves().whole - (rect(strip)?.w ?? 0),
  };
  click(strip);
  await until(() => inFront('[data-slot="tab-chat"]') !== null);
  seen.expanded = { view: chatOf(PAGE_TAB)!.view, id: chatOf(PAGE_TAB)!.id === chat.id, share: chatOf(PAGE_TAB)!.share };

  // Hide closes the split, the button comes back; it opens the same chat at the same width.
  click(inFront('[data-slot="tab-chat-header"] button:nth-of-type(3)'));
  await until(() => inFront('[data-slot="tab-chat"]') === null && button() !== null);
  seen.hidden = { view: chatOf(PAGE_TAB)!.view, button: button(), pageFull: halves().content === halves().whole };
  // While hidden, the dot shows while the chat works, and for what came since it was seen.
  useActivity.setState((state) => ({ byConversation: { ...state.byConversation, [chat.id]: { run: "running", tasks: {}, approvals: new Set(), questions: new Set() } } }));
  await until(() => button()?.dot === true, 2000);
  seen.dotWorking = button()?.dot;
  useActivity.setState((state) => ({ byConversation: { ...state.byConversation, [chat.id]: { run: "idle", tasks: {}, approvals: new Set(), questions: new Set() } } }));
  await until(() => button()?.dot === false, 2000);
  seen.dotIdle = button()?.dot;
  useApp.setState((state) => ({ conversations: { ...state.conversations, [chat.id]: { ...state.conversations[chat.id]!, updatedAtMs: (chatOf(PAGE_TAB)!.seenAt ?? 0) + 1000 } } }));
  await until(() => button()?.dot === true, 2000);
  seen.dotNews = button()?.dot;
  click(inFront('[data-slot="tab-chat-button"] button'));
  await until(() => inFront('[data-slot="tab-chat"]') !== null);
  await wait(100);
  seen.reopened = { id: chatOf(PAGE_TAB)!.id === chat.id, share: chatOf(PAGE_TAB)!.share, halves: halves() };

  // New chat starts over in the same half; the old one is deleted.
  const before = asked.length;
  click(inFront('[data-slot="tab-chat-header"] button:nth-of-type(1)'));
  await until(() => chatOf(PAGE_TAB)!.id !== chat.id);
  await wait(100);
  seen.newChat = {
    fresh: chatOf(PAGE_TAB)!.id !== chat.id, view: chatOf(PAGE_TAB)!.view, share: chatOf(PAGE_TAB)!.share,
    deleted: asked.slice(before).some((request) => request.method === "delete" && request.ids.includes(chat.id)),
    opened: asked.slice(before).some((request) => request.method === "openTabChat" && request.chatId === chatOf(PAGE_TAB)!.id),
  };

  // A file tab's chat is about the file.
  selectTab(ID, "file:src/app.ts");
  await until(() => button() !== null);
  click(inFront('[data-slot="tab-chat-button"] button'));
  await until(() => inFront('[data-slot="tab-chat"] [data-embedded-view]') !== null);
  await wait(100);
  seen.fileChat = {
    target: asked.findLast((request) => request.method === "openTabChat")?.method === "openTabChat"
      ? (asked.findLast((request) => request.method === "openTabChat") as Extract<Request, { method: "openTabChat" }>).target : null,
    welcome: inFront('[data-slot="tab-chat"] [data-embedded-view]')?.textContent?.includes("Ask about this file") ?? false,
  };
  return seen;
}

function Fixture() {
  const [result, setResult] = useState("");
  useEffect(() => {
    if (!query.has("drive")) return;
    void drive()
      .then((seen) => setResult(JSON.stringify(seen)))
      .catch((error: unknown) => setResult(JSON.stringify({ error: String(error instanceof Error ? error.stack : error) })));
  }, []);
  return (
    <>
      <App />
      {result && <pre id="tab-chat-result" hidden>{result}</pre>}
    </>
  );
}

createRoot(document.getElementById("root")!).render(<Fixture />);
