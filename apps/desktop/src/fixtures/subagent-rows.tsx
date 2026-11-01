/**
 * A standalone Vite entry, excluded from the app entry and production build: a session whose
 * lead ran a subagent it waited for, then one in the background beside a background command,
 * and ended its turn. Its block's rows, the side panel a subagent's row opens, and the summary
 * card's Background section are read into `#subagent-rows-result`: while the background ones
 * run, once they ended, and once its CLI no longer lists them.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { ConversationView } from "@/app/conversation/ConversationView";
import { SidebarProvider } from "@/components/layout";
import { TooltipProvider } from "@/components/overlays";
import type { BackgroundTask, Conversation, FileEntry, ProviderEvent, Request, ThreadView } from "@/ipc/generated";
import { applyActivityNews } from "@/state/activity";
import { emptyBoard, useBoard } from "@/state/board";
import { useCliThread } from "@/state/cliThread";
import { emptyThread, useApp } from "@/state/store";

const id = "subagent-rows";
const t0 = Date.now() - 60_000;
const at = (offset: number) => t0 + offset;
const entry = (event: object, offset: number, more: Partial<FileEntry> = {}): FileEntry => ({ event: event as ProviderEvent, atMs: at(offset), ...more });
const said = (itemId: string, role: "user" | "assistant", text: string, offset: number, parent?: string) =>
  entry({ type: "message", itemId, role, text }, offset, parent ? { parent } : {});
const call = (itemId: string, name: string, input: object, offset: number, ranMs: number) =>
  entry({ type: "toolCall", itemId, name, input: JSON.stringify(input), status: "completed", output: "" }, offset, { updatedAtMs: at(offset + ranMs) });

const entries: FileEntry[] = [
  said("u1", "user", "Carry out the plan", 0),
  // Waited for: its call ran as long as it did.
  call("fg", "Agent", { description: "Carry out TASK.md steps", prompt: "Carry out the steps in TASK.md." }, 1_000, 15_000),
  said("fg1", "assistant", "All TASK.md steps are done.", 10_000, "fg"),
  // In the background: its call answered at once, its task runs on.
  call("bg", "Agent", { description: "Explore the parser", prompt: "Read the parser.", run_in_background: true }, 17_000, 100),
  said("bg1", "assistant", "Reading the parser now.", 18_000, "bg"),
  call("cmd", "Bash", { command: "sleep 30", run_in_background: true }, 17_200, 100),
  said("a1", "assistant", "Both run in the background.", 19_000),
];
const task = (taskId: string, itemId: string, kind: BackgroundTask["kind"], label: string, offset: number, endedAt: number | null): BackgroundTask => ({
  id: taskId, itemId, kind, label, status: endedAt === null ? "running" : "completed", startedAtMs: at(offset), endedAtMs: endedAt === null ? null : at(endedAt),
  outputFile: null, summary: endedAt === null || kind === "agent" ? null : "Background command \"sleep 30\" completed (exit code 0)", lastLine: null,
  ...(kind === "agent" ? { provider: "claude" } : { command: "sleep 30" }),
}) as BackgroundTask;
const turns = [{ userMessage: "u1", startedAtMs: at(0), endedAtMs: at(20_000), end: "done", error: null, requests: ["r1"] }] as ThreadView["turns"];

let view: ThreadView = {
  entries, turns,
  background: [task("b1", "bg", "agent", "Explore the parser", 17_000, null), task("s1", "cmd", "shell", "sleep 30", 17_200, null)],
};

const conversation = {
  id, kind: "session", projectId: null, title: "Carry out the plan", pinnedAtMs: null, createdAtMs: t0, updatedAtMs: at(20_000),
  lifecycle: "active", forkedFrom: null, sideOf: null, quotaWait: null,
  setup: {
    type: "session", repo: "/tmp/subagent-rows", environment: { type: "newWorktree", branch: "brigadier/rows/session", base: "main" },
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
  summaryShown: { [id]: true },
  connection: { status: "connected", daemon: null, reason: null },
  info: { platform: "macos", version: "fixture", processStartMs: 0, smoke: false, firstLaunch: false, budgetTolerance: 1 },
});
applyActivityNews({ conversationId: id, entries: [], ended: "done" });
useCliThread.setState((state) => ({ views: { ...state.views, [id]: view } }));

const pause = () => new Promise((resolve) => setTimeout(resolve, 400));
const show = (next: ThreadView) => {
  view = next;
  useCliThread.setState((state) => ({ views: { ...state.views, [id]: next } }));
};
const text = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, " ").trim() ?? null;

/** Opens every closed work group and folded work of the thread, so each step's row shows. */
async function openWork() {
  for (const header of document.querySelectorAll<HTMLElement>('button[data-slot="request-work-header"][aria-expanded="false"]')) header.click();
  await pause();
  for (const group of document.querySelectorAll<HTMLElement>('[data-slot="work-group"][data-state="closed"] > [role="button"], [data-slot="work-group"][data-state="closed"] > button')) group.click();
  await pause();
}

/** The block's step rows, the Workers panel's workers and the Background section's rows. */
function seen() {
  return {
    steps: [...document.querySelectorAll('[data-slot="orchestrator-step"]')].map(text),
    workers: [...document.querySelectorAll('[data-task], [data-slot="workers-strip-row"], [data-slot="workers-summary-worker"]')].map(text),
    background: document.querySelector("[data-activities]") ? [...document.querySelectorAll("[data-activities] [data-activity]")].map(text) : null,
    panel: text(document.querySelector('[data-slot="worker-thread"]')),
  };
}

/** Clicks the first element `selector` finds whose words match `words`. */
async function click(selector: string, words: RegExp) {
  [...document.querySelectorAll<HTMLElement>(selector)].find((element) => words.test(text(element) ?? ""))?.click();
  await pause();
}

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
  await openWork();
  const running = seen();
  // A subagent's row opens its transcript in the side panel.
  await click('[data-slot="orchestrator-step"]', /^Ran a subagent: Carry out/);
  const foreground = seen().panel;
  // So does a background one's Background row.
  await click("[data-activities] [data-activity]", /^Subagent:/);
  const background = seen().panel;
  // Both end.
  show({ ...view, background: [task("b1", "bg", "agent", "Explore the parser", 17_000, 40_000), task("s1", "cmd", "shell", "sleep 30", 17_200, 47_200)] });
  await pause();
  await openWork();
  const ended = seen();
  // Its CLI lists them no more.
  show({ ...view, background: [] });
  await pause();
  const forgotten = seen();
  await click("[data-activities] [data-activity]", /^Subagent:/);
  const reopened = seen().panel;
  const result = document.createElement("pre");
  result.id = "subagent-rows-result";
  result.textContent = JSON.stringify({ running, foreground, background, ended, forgotten, reopened });
  document.body.append(result);
}

createRoot(document.getElementById("root")!).render(<Page />);
void drive();
