/**
 * The context card's pull request row with its buttons, per `?state=`: `failing` (CI failing),
 * `approved` (approved, a review thread unresolved), `conflicts`, `draft`, `merged`. It writes the
 * buttons it shows to #pr-card-result; `?click=<label>` clicks that one twice first and records
 * what was sent (PrCardRender.test.ts reads it).
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { PinnedSummary } from "@/app/conversation/PinnedSummary";
import { SidebarProvider } from "@/components/layout/sidebar";
import { TooltipProvider } from "@/components/overlays/tooltip";
import type { Conversation, PullRequest, Request } from "@/ipc/generated";
import { emptyBoard, useBoard } from "@/state/board";
import { useApp } from "@/state/store";

const id = "pr-card-fixture";
const params = new URLSearchParams(location.search);
const conversation: Conversation = {
  id, kind: "session", title: "Add notes", projectId: null, pinnedAtMs: null, createdAtMs: 0, updatedAtMs: 0,
  setup: { type: "session", repo: "/fixture", environment: { type: "localCheckout", branch: "add-notes" }, permission: "askForApproval", planMode: false, workersSeeUncommitted: null, orchestrator: { provider: "claude", model: null, effort: null } },
  lifecycle: "active", forkedFrom: null, sideOf: null, fallback: null, quotaWait: null,
};
const base: PullRequest = { number: 7, title: "Add notes", url: "https://github.com/o/n/pull/7", state: "open", base: "main", ci: "passing" };
const STATES: Record<string, PullRequest> = {
  failing: { ...base, ci: "failing" },
  approved: { ...base, unresolved: 1 },
  conflicts: { ...base, conflicts: true, changesRequested: true },
  draft: { ...base, state: "draft", ci: "pending" },
  merged: { ...base, state: "merged", mergeCommit: "f00dfeed1234" },
};
useBoard.setState({ board: { ...emptyBoard(id), loaded: true } });
useApp.setState({
  summaryShown: { [id]: true }, selection: { type: "conversation", id }, conversations: { [id]: conversation },
  info: { platform: "macos", version: "fixture", processStartMs: 0, smoke: false, firstLaunch: false, budgetTolerance: 1 },
  connection: { status: "connected", daemon: null, reason: null },
});
const sent: string[] = [];
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  if (req.method === "getPullRequest") return { method: req.method, pullRequest: STATES[params.get("state") ?? "approved"] ?? null };
  if (req.method === "sendMessage") {
    sent.push(req.text);
    const message = { id: `sent-${sent.length}`, conversationId: id, seq: sent.length, role: "user", text: req.text, blob: null, createdAtMs: Date.now(), attachments: [], mentions: [], model: null, requestId: null, parentId: null };
    return new Promise((resolve) => setTimeout(() => resolve({ method: req.method, outcome: { type: "sent", message } }), 200));
  }
  return { method: req.method };
});

const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const row = () => [...document.querySelectorAll<HTMLElement>('[data-slot="summary-card"] button')].find((button) => button.textContent?.includes("#7"));
const buttons = () => [...document.querySelectorAll<HTMLButtonElement>('[data-slot="pr-actions"] button')];

async function drive() {
  for (let tries = 0; tries < 200 && !row(); tries++) await wait(50);
  await wait(200);
  const seen: Record<string, unknown> = { row: !!row(), shown: buttons().map((button) => button.textContent?.trim()) };
  const label = params.get("click");
  if (label) {
    const button = () => buttons().find((each) => each.textContent?.trim() === label);
    button()?.click();
    button()?.click();
    for (let tries = 0; tries < 100 && !button()?.disabled; tries++) await wait(50);
    await wait(400);
    seen.sent = sent;
    seen.asked = button()?.disabled ?? null;
  }
  const result = document.createElement("pre");
  result.id = "pr-card-result";
  result.textContent = JSON.stringify(seen);
  document.body.append(result);
}

createRoot(document.getElementById("root")!).render(
  <TooltipProvider>
    <SidebarProvider defaultOpen={false}>
      <main className="bg-background text-foreground relative flex h-screen w-full min-w-0">
        <div className="flex min-w-0 flex-1 flex-col">
          <PinnedSummary conversation={conversation} />
        </div>
      </main>
    </SidebarProvider>
  </TooltipProvider>,
);
void drive();
