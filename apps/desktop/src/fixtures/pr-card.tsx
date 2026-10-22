/**
 * The context card's pull request row, per `?state=`: `failing` (checks failed), `approved` (open,
 * checks passing), `conflicts`, `draft`, `merged`. It writes what the row shows to
 * #pr-card-result: its words, its state word and tone, and any buttons it has (none, S14). With
 * `?push=1` its thread then finishes a `git push`, and the result counts the row's reads of `gh`.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { PinnedSummary } from "@/app/conversation/PinnedSummary";
import { SidebarProvider } from "@/components/layout";
import { TooltipProvider } from "@/components/overlays";
import type { Conversation, FileEntry, PullRequest, Request } from "@/ipc/generated";
import { emptyBoard, useBoard } from "@/state/board";
import { useCliThread } from "@/state/cliThread";
import { useApp } from "@/state/store";

const id = "pr-card-fixture";
const params = new URLSearchParams(location.search);
const conversation: Conversation = {
  id, kind: "session", title: "Add notes", projectId: null, pinnedAtMs: null, createdAtMs: 0, updatedAtMs: 0,
  setup: { type: "session", repo: "/fixture", environment: { type: "localCheckout", branch: "add-notes" }, permission: "ask", planMode: false, workersSeeUncommitted: null, orchestrator: { provider: "claude", model: null, effort: null } },
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
const entries: FileEntry[] = [{ event: { type: "message", itemId: "u1", role: "user", text: "Open a pull request" } }];
useCliThread.setState({ views: { [id]: { entries, turns: [] } } });
useApp.setState({
  summaryShown: { [id]: true }, selection: { type: "conversation", id }, conversations: { [id]: conversation },
  info: { platform: "macos", version: "fixture", processStartMs: 0, smoke: false, firstLaunch: false, budgetTolerance: 1 },
  connection: { status: "connected", daemon: null, reason: null },
});
let reads = 0;
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  if (req.method === "getPullRequest") {
    reads += 1;
    return { method: req.method, pullRequest: STATES[params.get("state") ?? "approved"] ?? null };
  }
  return { method: req.method };
});

const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const row = () => document.querySelector<HTMLElement>('[data-slot="pull-request-row"]');

/** A command of the thread, finished. */
const ran = (itemId: string, command: string): FileEntry =>
  ({ event: { type: "command", itemId, command, cwd: null, status: "completed", exitCode: 0, output: "", durationMs: 10 } });
const add = (entry: FileEntry) =>
  useCliThread.setState((state) => ({ views: { ...state.views, [id]: { ...state.views[id]!, entries: [...state.views[id]!.entries, entry] } } }));

async function drive() {
  for (let tries = 0; tries < 200 && !row(); tries++) await wait(50);
  await wait(200);
  const shown = row();
  const word = shown?.querySelector<HTMLElement>('[data-slot="pull-request-state"]');
  const seen: Record<string, unknown> = {
    row: !!shown,
    text: shown?.textContent?.replace(/\s+/g, " ").trim(),
    word: word?.textContent,
    tone: shown?.dataset.tone,
    red: !!word?.classList.contains("text-destructive"),
    buttons: shown?.parentElement?.querySelectorAll('[data-slot="pr-actions"], [data-slot="pr-actions"] button').length ?? 0,
  };
  if (params.has("push")) {
    const before = reads;
    // A command that is no push reads nothing again; a push does.
    add(ran("c1", "git status"));
    await wait(300);
    seen.afterStatus = reads - before;
    add(ran("c2", "/bin/zsh -lc 'git push -u origin HEAD'"));
    await wait(300);
    seen.afterPush = reads - before;
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
