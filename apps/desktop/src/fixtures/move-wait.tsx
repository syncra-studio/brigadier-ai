/**
 * A standalone Vite entry, excluded from the app entry and production build: a session's switch to
 * Codex waiting for its background jobs, IPC mocked. The composer says so; Switch now warns
 * first, Keep waiting asks nothing of the daemon, and Switch now in the warning does. With
 * `?stop=row` or `?stop=dialog` it stays there, to be looked at. What it saw goes as JSON into
 * `<pre id="move-wait-result">`.
 */
import { mockIPC } from "@/ipc/nativeMocks";
import { createRoot } from "react-dom/client";

import { ConversationView } from "@/app/conversation/ConversationView";
import { SidebarProvider } from "@/components/layout";
import { TooltipProvider } from "@/components/overlays";
import { model, overview } from "@/fixtures/agents";
import type { Conversation, ConversationView as View, Request } from "@/ipc/generated";
import { select } from "@/state/actions";
import { useApp } from "@/state/store";

const id = "move-wait";
const start = Date.now() - 20 * 60_000;
const conversation = {
  id, kind: "session", projectId: null, title: "Tax helpers", pinnedAtMs: null, createdAtMs: start, updatedAtMs: start,
  lifecycle: "active", forkedFrom: null, sideOf: null, quotaWait: null,
  setup: {
    type: "session", orchestrator: { provider: "claude", model: "opus", effort: "high" }, permission: "acceptEdits", planMode: false,
    workersSeeUncommitted: null, environment: { type: "localCheckout", repoPath: "/repo", branch: null },
  },
} as unknown as Conversation;

const asked: unknown[] = [];
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  switch (req.method) {
    case "getConversation":
      return {
        method: req.method,
        view: {
          conversation, context: null, messages: { messages: [], hasMore: false }, tasks: [], approvals: [], questions: [],
          requests: [], workerSteps: [], orchestratorSteps: [], machineSteps: [], thinking: [],
          compactions: [], decisions: [], queue: { items: [], paused: false }, run: "idle",
          runRequest: null, head: null, ratings: {}, streaming: null, notices: [], previews: [],
        } as unknown as View,
      };
    case "threadView":
      return { method: req.method, view: { entries: [], turns: [], jobs: { holding: ["job"], moveTo: "codex" } } };
    case "moveNow":
      asked.push(req.id);
      return { method: req.method };
    default:
      return { method: req.method };
  }
});
const state = useApp.getState();
useApp.setState({
  conversations: { [id]: conversation },
  connection: { status: "connected", daemon: null, reason: null },
  providers: {
    ...state.providers,
    view: {
      providers: [
        overview("claude", [model("opus", "Opus 5.5", { isDefault: true })]),
        overview("codex", [model("gpt-5.5", "GPT-5.5", { isDefault: true })]),
      ],
      sessions: [],
      fixtures: [],
    },
  },
});

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
async function until<T>(what: string, find: () => T | null | undefined | false, ms = 20_000): Promise<T> {
  const end = performance.now() + ms;
  for (;;) {
    const found = find();
    if (found) return found;
    if (performance.now() > end) throw new Error(`timed out waiting for ${what}`);
    await sleep(16);
  }
}
const row = () => document.querySelector<HTMLElement>("[data-slot=move-wait]");
const buttons = (within: ParentNode) => [...within.querySelectorAll<HTMLButtonElement>("button")];
const named = (within: ParentNode, name: string) => buttons(within).find((button) => button.textContent === name);
const dialog = () => document.querySelector<HTMLElement>("[role=dialog]");
const stop = new URLSearchParams(location.search).get("stop");

async function drive(): Promise<void> {
  const seen: Record<string, unknown> = {};
  try {
    seen.row = (await until("the row", row)).textContent;
    if (stop === "row") return;
    named(row()!, "Switch now")!.click();
    seen.warning = (await until("the warning", dialog)).textContent;
    seen.askedBeforeAnswer = asked.length;
    if (stop === "dialog") return;
    named(dialog()!, "Keep waiting")!.click();
    await until("the warning gone", () => !dialog());
    seen.askedAfterKeepWaiting = asked.length;
    named(row()!, "Switch now")!.click();
    await until("the warning again", dialog);
    named(dialog()!, "Switch now")!.click();
    await until("the switch asked for", () => asked.length > 0);
    await until("the warning gone again", () => !dialog());
    seen.asked = asked;
    seen.rowAfter = (await until("the row saying so", () => row()?.textContent?.includes("…") && row())).textContent;
    seen.buttonAfter = named(row()!, "Switch now") !== undefined;
  } catch (error) {
    seen.error = String(error);
  }
  const out = document.createElement("pre");
  out.id = "move-wait-result";
  out.textContent = JSON.stringify(seen);
  document.body.append(out);
}

select({ type: "conversation", id });
createRoot(document.getElementById("root")!).render(
  <TooltipProvider>
    <SidebarProvider className="bg-background flex h-screen flex-col" defaultOpen={false}>
      <main className="relative flex min-h-0 w-full flex-1 flex-col">
        <ConversationView key={id} selection={{ type: "conversation", id }} />
      </main>
    </SidebarProvider>
  </TooltipProvider>,
);
void drive();
