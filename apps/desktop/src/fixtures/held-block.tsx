/**
 * A standalone Vite entry, excluded from the app entry and production build: a session's block
 * held by its workers. Its lead starts a worker and ends its turn; the user writes twice while
 * the workers work (the second turn starts another), each message a turn that joins the block;
 * then the workers finish one by one, and their word joins it too. Each step's work headers
 * and blocks are read into `#held-block-result`.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { ConversationView } from "@/app/conversation/ConversationView";
import { SidebarProvider } from "@/components/layout";
import { TooltipProvider } from "@/components/overlays";
import type { Conversation, FileEntry, ProviderEvent, Request, ThreadTurn, ThreadView, WorkerRow } from "@/ipc/generated";
import { applyActivityNews } from "@/state/activity";
import { emptyBoard, useBoard } from "@/state/board";
import { useCliThread } from "@/state/cliThread";
import { emptyThread, useApp } from "@/state/store";

const id = "held-session";
const t0 = Date.now() - 10 * 60_000;
const at = (offset: number) => t0 + offset;
const said = (itemId: string, role: "user" | "assistant", text: string, offset: number): FileEntry => ({
  event: { type: "message", itemId, role, text } as ProviderEvent,
  atMs: at(offset),
});
const turn = (userMessage: string, from: number, to: number, requests: string[], joins = false): ThreadTurn => ({
  userMessage, startedAtMs: at(from), endedAtMs: at(to), end: "done", error: null, requests, ...(joins && { joins }),
});
const worker = (wid: string, title: string, offset: number, done: boolean): WorkerRow => ({
  id: wid, number: wid === "aa11bb22" ? 1 : 2, title, agent: "claude", model: null, startedAtMs: at(offset), working: !done,
  lastEnd: done ? "done" : null, gone: false, lastEventAtMs: null, limit: null, freshContext: false, waiting: [], mcp: [],
  branch: null, base: null, diff: null, ahead: 0, merged: false,
});

const entries: FileEntry[] = [
  said("u1", "user", "Fix the two bugs", 0),
  said("a1", "assistant", "A worker is on the first.", 2_000),
  // Sent while the worker worked: its turn joins the block, and starts the second worker.
  said("u2", "user", "Also keep it fast", 60_000),
  said("a2", "assistant", "Another worker takes the second.", 62_000),
  // Sent while both work.
  said("u3", "user", "How is it going?", 120_000),
  said("a3", "assistant", "Both are still working.", 122_000),
];
const turns: ThreadTurn[] = [
  turn("u1", 0, 3_000, ["r1"]),
  turn("u2", 60_000, 63_000, ["r2"], true),
  turn("u3", 120_000, 123_000, ["r3"], true),
];
const news = (itemId: string, wid: string, offset: number): FileEntry =>
  said(itemId, "user", `[worker events wn-0123456789ab] Worker "Fix" (${wid}): finished · Done. (ev-1234abcd)`, offset);

let view: ThreadView = {
  entries,
  turns,
  workers: [worker("aa11bb22", "First bug", 1_000, false), worker("cc33dd44", "Second bug", 61_000, false)],
};

const conversation = {
  id, kind: "session", projectId: null, title: "Fix the two bugs", pinnedAtMs: null, createdAtMs: t0, updatedAtMs: at(123_000),
  lifecycle: "active", forkedFrom: null, sideOf: null, quotaWait: null,
  setup: {
    type: "session", repo: "/tmp/held-fixture", environment: { type: "newWorktree", branch: "brigadier/held/session", base: "main" },
    permission: "full", planMode: false, workersSeeUncommitted: null, orchestrator: { provider: "claude", model: "opus", effort: "high" },
  },
} as unknown as Conversation;

mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  switch (req.method) {
    case "threadView":
      return { method: req.method, view };
    case "getPullRequest":
      return { method: req.method, pullRequest: null };
    case "listFiles":
      return { method: req.method, files: [], truncated: false };
    default:
      return { method: req.method };
  }
});

useBoard.setState({ board: { ...emptyBoard(id), loaded: true } });
useApp.setState({
  selection: { type: "conversation", id },
  conversations: { [id]: conversation },
  threads: { [id]: emptyThread },
  summaryShown: { [id]: false },
  connection: { status: "connected", daemon: null, reason: null },
});
applyActivityNews({ conversationId: id, entries: [], ended: "done" });
useCliThread.setState((state) => ({ views: { ...state.views, [id]: view } }));

const pause = () => new Promise((resolve) => setTimeout(resolve, 300));
const show = (next: ThreadView) => {
  view = next;
  useCliThread.setState((state) => ({ views: { ...state.views, [id]: next } }));
};
/** The thread as drawn: each block's header, and the steered bubbles in it. */
function seen() {
  const blocks = [...document.querySelectorAll('[data-slot="aui_assistant-message-root"]')];
  return {
    blocks: blocks.length,
    headers: blocks.map((block) => block.querySelector('[data-slot="request-work-label"]')?.textContent ?? null),
    steers: blocks.map((block) => [...block.querySelectorAll('[data-slot="request-steer"]')].map((steer) => steer.textContent?.replace(/\s*\d.*$/, "").trim() ?? "")),
  };
}

/** The conversation in the app's own layout (App.tsx), without its sidebar and dialogs. */
function Page() {
  return (
    <TooltipProvider>
      <SidebarProvider className="flex h-screen flex-col" defaultOpen={false}>
        <main className="relative flex h-full min-w-0 flex-1">
          <div className="relative flex h-full min-w-0 flex-1 flex-col">
            <ConversationView selection={{ type: "conversation", id }} />
          </div>
        </main>
      </SidebarProvider>
    </TooltipProvider>
  );
}

async function drive() {
  await pause();
  const bothWork = seen();
  // The first worker finishes; its word joins the block.
  show({
    ...view,
    entries: [...entries, news("n1", "aa11bb22", 200_000), said("a4", "assistant", "The first bug is fixed.", 201_000)],
    turns: [...turns, turn("n1", 200_000, 202_000, [])],
    workers: [worker("aa11bb22", "First bug", 1_000, true), worker("cc33dd44", "Second bug", 61_000, false)],
  });
  await pause();
  const oneWorks = seen();
  // The second finishes too: the block's work is over, and folds under its answer.
  show({
    ...view,
    entries: [...view.entries, news("n2", "cc33dd44", 300_000), said("a5", "assistant", "Both bugs are fixed.", 301_000)],
    turns: [...view.turns, turn("n2", 300_000, 302_000, [])],
    workers: [worker("aa11bb22", "First bug", 1_000, true), worker("cc33dd44", "Second bug", 61_000, true)],
  });
  await pause();
  const done = seen();
  const result = document.createElement("pre");
  result.id = "held-block-result";
  result.textContent = JSON.stringify({ bothWork, oneWorks, done });
  document.body.append(result);
}

createRoot(document.getElementById("root")!).render(<Page />);
void drive();
