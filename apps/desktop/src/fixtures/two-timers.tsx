/**
 * A standalone Vite entry, excluded from the app entry and production build: a session whose
 * message was sent two seconds before its CLI's turn began (D94). While it works, the work
 * block's header and the composer's placeholder are read together, several times across the
 * seconds, into `#two-timers-result`. Then its turn ends while a background subagent it started
 * still runs (D24): the two are read together again while that holds the block open, and once
 * the subagent ended, into `#two-timers-held`.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { ConversationView } from "@/app/conversation/ConversationView";
import { SidebarProvider } from "@/components/layout";
import { TooltipProvider } from "@/components/overlays";
import type { Conversation, Message, ProviderEvent, Request, ThreadView } from "@/ipc/generated";
import { applyActivityNews } from "@/state/activity";
import { emptyBoard, useBoard } from "@/state/board";
import { useCliThread } from "@/state/cliThread";
import { emptyThread, useApp } from "@/state/store";

const id = "two-timers";
const sentAt = Date.now() - 12_000;
const turnAt = sentAt + 2_000;
const message = {
  id: "r1", conversationId: id, seq: 0, role: "user", text: "Read TASK.md", blob: null, createdAtMs: sentAt,
  attachments: [], mentions: [], model: null, requestId: "r1", parentId: null,
} as unknown as Message;
const view: ThreadView = {
  entries: [{ event: { type: "message", itemId: "u1", role: "user", text: "Read TASK.md" } as ProviderEvent, atMs: turnAt, requests: ["r1"] }],
  turns: [{ userMessage: "u1", startedAtMs: turnAt, endedAtMs: null, end: null, error: null, requests: ["r1"] }],
} as unknown as ThreadView;
const conversation = {
  id, kind: "session", projectId: null, title: "Read TASK.md", pinnedAtMs: null, createdAtMs: sentAt, updatedAtMs: sentAt,
  lifecycle: "active", forkedFrom: null, sideOf: null, quotaWait: null,
  setup: {
    type: "session", repo: "/tmp/two-timers-fixture", environment: { type: "newWorktree", branch: "brigadier/tt/session", base: "main" },
    permission: "acceptEdits", planMode: false, workersSeeUncommitted: null, orchestrator: { provider: "claude", model: "opus", effort: "high" },
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

useBoard.setState({ board: { ...emptyBoard(id), loaded: true, run: "running" } });
useApp.setState({
  selection: { type: "conversation", id },
  conversations: { [id]: conversation },
  threads: { [id]: { ...emptyThread, items: [message] } },
  summaryShown: { [id]: false },
  connection: { status: "connected", daemon: null, reason: null },
});
applyActivityNews({ conversationId: id, entries: [], opened: true });
useCliThread.setState((state) => ({ views: { ...state.views, [id]: view } }));

const pause = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const header = () => document.querySelector('[data-slot="request-work-label"]')?.textContent ?? null;
const placeholder = () => document.querySelector("[data-slot=aui_composer-shell] [contenteditable=true]")?.getAttribute("aria-placeholder") ?? null;

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
  await pause(600);
  // Read together, every 350 ms for about 3 s: across a few ticks, at different points of each.
  const reads: [string | null, string | null][] = [];
  for (let i = 0; i < 9; i++) {
    reads.push([header(), placeholder()]);
    await pause(350);
  }
  const result = document.createElement("pre");
  result.id = "two-timers-result";
  result.textContent = JSON.stringify(reads);
  document.body.append(result);

  // Its turn ends; the subagent it started in the background runs on.
  const agent = { id: "a1", itemId: null, kind: "agent", label: "Sleep then print", status: "running", startedAtMs: turnAt + 1_000, endedAtMs: null, outputFile: null, summary: null, lastLine: null };
  const endedAt = turnAt + 5_000;
  const reply = { event: { type: "message", itemId: "a-1", role: "assistant", text: "Started a subagent in the background." } as ProviderEvent, atMs: endedAt - 500 };
  const ended = { ...view, entries: [...view.entries, reply], turns: [{ ...view.turns[0]!, end: "done", endedAtMs: endedAt }], background: [agent] } as unknown as ThreadView;
  useBoard.setState({ board: { ...emptyBoard(id), loaded: true, run: "idle" } });
  useCliThread.setState((state) => ({ views: { ...state.views, [id]: ended } }));
  applyActivityNews({ conversationId: id, entries: [], opened: false, ended: "done", turnEndedAtMs: endedAt } as never);
  await pause(600);
  const held: [string | null, string | null][] = [];
  for (let i = 0; i < 4; i++) {
    held.push([header(), placeholder()]);
    await pause(350);
  }
  // The subagent ends: no turn follows it (as on Codex).
  const done = { ...ended, background: [{ ...agent, status: "completed", endedAtMs: Date.now() }] } as unknown as ThreadView;
  useCliThread.setState((state) => ({ views: { ...state.views, [id]: done } }));
  await pause(600);
  const after = [header(), placeholder()];
  const out = document.createElement("pre");
  out.id = "two-timers-held";
  out.textContent = JSON.stringify({ held, after });
  document.body.append(out);
}

createRoot(document.getElementById("root")!).render(<Page />);
void drive();
