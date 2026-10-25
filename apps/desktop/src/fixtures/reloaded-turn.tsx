/**
 * A standalone Vite entry, excluded from the app entry and production build: a Codex session
 * opened after a restart, its thread read back from the CLI's rollout as the daemon reads it
 * (the view `a_codex_turn_the_stop_hook_sent_back_reads_back_after_a_restart` makes). The turn
 * the Stop hook sent back once (harness D49) asks on the card and ends in an empty reply (D58).
 * IPC is mocked, no daemon. What the turn shows goes to `<pre id="reloaded-turn-result">`.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { ConversationView } from "@/app/conversation/ConversationView";
import { SidebarProvider } from "@/components/layout";
import { TooltipProvider } from "@/components/overlays";
import type { Conversation, ConversationView as View, Request, ThreadView } from "@/ipc/generated";
import sendBack from "../../../../crates/providers/fixtures/session-files/codex-hook-sendback.view.json";
import { openConversation } from "@/state/actions";
import { useApp } from "@/state/store";

const ID = "reloaded-turn";
const conversation = {
  id: ID, kind: "session", projectId: null, title: "X", pinnedAtMs: null, createdAtMs: 0, updatedAtMs: 0, lifecycle: "active",
  forkedFrom: null, sideOf: null, quotaWait: null,
  setup: {
    type: "session", repo: "/tmp/projX", environment: { type: "local" }, permission: "full", planMode: false,
    workersSeeUncommitted: null, orchestrator: { provider: "codex", model: "gpt-6-luna", effort: "low" },
  },
} as unknown as Conversation;

/** What the daemon says of the session when it opens: nothing beyond its CLI thread. */
const view = {
  conversation, context: null, messages: { messages: [], hasMore: false },
  tasks: [], approvals: [], questions: [], requests: [], workerSteps: [], orchestratorSteps: [], machineSteps: [], thinking: [], compactions: [], decisions: [],
  queue: { items: [], paused: false }, run: "idle", runRequest: null, head: null, ratings: {}, streaming: null, notices: [], previews: [],
} as unknown as View;

mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  switch (req.method) {
    case "getConversation": return { method: req.method, view };
    case "threadView": return { method: req.method, view: sendBack as unknown as ThreadView };
    case "listFiles": return { method: req.method, files: [], truncated: false };
    default: return { method: req.method };
  }
});

useApp.setState({
  conversations: { [ID]: conversation },
  connection: { status: "connected", daemon: null, reason: null },
  summaryShown: { [ID]: false },
});
await openConversation(ID);

function Page() {
  const selection = useApp((s) => s.selection);
  return (
    <TooltipProvider>
      <SidebarProvider className="bg-background flex h-screen flex-col" defaultOpen={false}>
        <main className="relative flex min-h-0 w-full flex-1 flex-col">
          <ConversationView selection={selection} />
        </main>
      </SidebarProvider>
    </TooltipProvider>
  );
}
createRoot(document.getElementById("root")!).render(<Page />);

const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const text = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, " ").trim() ?? null;
const turn = () => [...document.querySelectorAll("[data-role=assistant]")].at(-1) ?? null;

for (let tries = 0; tries < 100 && !turn(); tries++) await wait(50);
await wait(300);
const shown = turn();
const result = document.createElement("pre");
result.id = "reloaded-turn-result";
result.textContent = JSON.stringify({
  turns: document.querySelectorAll("[data-role=assistant]").length,
  failed: text(document.querySelector("[data-slot=turn-render-error]")),
  answer: text(shown?.querySelector("[data-slot=aui_assistant-message-content][data-answer]")),
  text: text(shown),
  page: text(document.querySelector("main"))?.slice(0, 400),
});
document.body.append(result);
