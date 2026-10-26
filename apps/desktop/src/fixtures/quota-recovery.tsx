/** Real conversation/composer recovery states. Mock IPC only; no daemon or model calls. */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";
import { ConversationView } from "@/app/conversation/ConversationView";
import { SidebarProvider } from "@/components/layout";
import { TooltipProvider } from "@/components/overlays";
import { fixtureClick, fixtureInput } from "@/fixtures/input";
import night from "@/fixtures/boards/overnight-2026-10-03.json";
import type { Conversation, ConversationView as View, ModelInfo, ProviderOverview, QuotaWait, Request, Task, ThreadView } from "@/ipc/generated";
import { openConversation } from "@/state/actions";
import { useBoard } from "@/state/board";
import { useApp } from "@/state/store";
import { useToasts } from "@/state/toasts";

const params = new URLSearchParams(location.search);
const ID = "quota-recovery";
const now = Date.now();
const RESET = now + 95 * 60_000;
const wait: QuotaWait = { reason: "All accounts allowed by your routing rules are at their usage limits.",
  resetsAtMs: params.has("unknown") ? null : RESET, sinceMs: now, rule: null, ranking: null,
  limitKind: params.has("credits") ? "credits" : params.has("spend") ? "spendControl" : "usageWindow" };
const conversation = {
  id: ID, kind: "session", projectId: null, title: "Quota recovery", pinnedAtMs: null, createdAtMs: now, updatedAtMs: now,
  lifecycle: "active", forkedFrom: null, sideOf: null, quotaWait: wait,
  setup: { type: "session", repo: "/tmp/quota-fixture", environment: { type: "local" }, permission: "acceptEdits", planMode: false,
    workersSeeUncommitted: null, orchestrator: { provider: "claude", model: "opus", effort: null } },
} as unknown as Conversation;
const model = (id: string, displayName: string): ModelInfo => ({ id, displayName, description: "", resolved: null, efforts: [],
  defaultEffort: null, isDefault: true, inputModalities: ["text"], fast: null, legacy: false });
const providers = [
  { provider: "claude", status: null, models: { models: [model("opus", "Opus"), model("sonnet", "Sonnet")] } },
  { provider: "codex", status: null, models: { models: [model("gpt", "GPT")] } },
] as unknown as ProviderOverview[];
const view = {
  conversation, context: null, messages: { messages: [], hasMore: false }, tasks: [], approvals: [], questions: [], requests: [],
  workerSteps: [], orchestratorSteps: [{ requestId: "r1", atMs: now, position: 1, kind: { type: "tool", name: "Read", status: "inProgress" } }],
  machineSteps: [], thinking: [], compactions: [], decisions: [], queue: { items: [], paused: false },
  run: "running", runRequest: "r1", head: null, ratings: {}, streaming: null, notices: [], previews: [],
} as unknown as View;
const thread: ThreadView = { entries: [{ atMs: now, event: { type: "message", itemId: "u1", role: "user", text: "Continue the work." } },
  { atMs: now + 1, event: { type: "toolCall", itemId: "read", name: "Read", input: '{"file_path":"src/main.ts"}', status: "inProgress", output: null } }],
  turns: [{ userMessage: "u1", startedAtMs: now, endedAtMs: null, end: null, error: null, requests: ["r1"],
    limit: { kind: "usageWindow", resetsAtMs: RESET - 3_600_000, window: "five_hour" } }] };
const asked: Request[] = [];
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  switch (req.method) {
    case "getConversation": return { method: req.method, view };
    case "threadView": return { method: req.method, view: thread };
    case "listFiles": return { method: req.method, files: [], truncated: false };
    case "continueOn": asked.push(req); return { method: req.method, when: "afterTurn" };
    case "updateSetup": asked.push(req); return { method: req.method, conversation: { ...conversation, setup: req.setup } };
    case "continueWaiting": asked.push(req); throw new Error("Accounts are still at their limits. Try again later.");
    case "dontResume": asked.push(req); patchWait(null); return { method: req.method };
    case "sendMessage": asked.push(req); throw new Error("A picker must not send a message");
    default: return { method: req.method };
  }
});
function patchWait(quotaWait: QuotaWait | null) {
  useApp.setState((state) => ({ conversations: { ...state.conversations, [ID]: { ...state.conversations[ID]!, quotaWait } } }));
}
useApp.setState((state) => ({ conversations: { [ID]: conversation }, summaryShown: { [ID]: false },
  providers: { ...state.providers, view: { providers } as never }, connection: { status: "connected", daemon: null, reason: null } }));
await openConversation(ID);
function Page() {
  const selection = useApp((state) => state.selection);
  return <TooltipProvider><SidebarProvider className="bg-background flex h-screen flex-col" defaultOpen={false}>
    <main className="relative flex min-h-0 w-full flex-1 flex-col"><ConversationView selection={selection} /></main>
  </SidebarProvider></TooltipProvider>;
}
createRoot(document.getElementById("root")!).render(<Page />);
const sleep = (ms = 50) => new Promise((resolve) => setTimeout(resolve, ms));
async function until(what: string, check: () => boolean) {
  for (let tries = 0; tries < 200; tries++) { if (check()) return; await sleep(); }
  throw new Error(`Timed out: ${what}. ${document.querySelector("main")?.textContent}`);
}
const strip = () => document.querySelector<HTMLElement>("[data-slot=quota-wait-strip]");
const text = (el: Element | null) => el?.textContent?.replace(/\s+/g, " ").trim() ?? null;
const liveText = () => [...document.querySelectorAll("[data-slot=thinking-live], [data-slot=thread-status], [data-slot=group-live-line]")].map(text);
try {
  await until("quota strip and work header", () => !!strip() && !!document.querySelector("[data-slot=request-work-label]"));
  await sleep(300);
  if (!params.has("stop")) {
    const seen: Record<string, unknown> = { initial: text(strip()), strips: document.querySelectorAll("[data-slot=quota-wait-strip]").length,
      oldCards: document.querySelectorAll("[data-slot=schedule-card]").length, pills: document.querySelectorAll("[data-slot=quota-wait-pill]").length,
      initialLive: liveText(), header: text(document.querySelector("[data-slot=request-work-label]")),
      staleToolShimmers: document.querySelectorAll("[data-slot=orchestrator-step] .shimmer").length,
      pausedTools: document.querySelectorAll("[data-slot=orchestrator-step][data-step-state=paused]").length };
    // Current null reset must override the old turn's known reset.
    patchWait({ ...wait, resetsAtMs: null }); await sleep(); seen.unknown = text(strip());
    for (const kind of ["credits", "spendControl"] as const) {
      patchWait({ ...wait, limitKind: kind }); await sleep();
      seen[kind] = { text: text(strip()), cancels: !!strip()?.querySelector("[data-slot=dont-resume]") };
    }
    patchWait(wait); await sleep();
    const engine = { ...Object.values(night.tasks)[0], id: "engine", requestId: "r1", title: "Independent worker", state: "running", quotaWait: null,
      blockedReason: null, candidate: null, attempts: [], createdAtMs: now, updatedAtMs: now } as unknown as Task;
    useBoard.setState((s) => ({ board: { ...s.board!, tasks: { engine } } }));
    await sleep(1200); seen.workerLive = liveText(); seen.workerHeader = text(document.querySelector("[data-slot=request-work-label]"));
    useBoard.setState((s) => ({ board: { ...s.board!, tasks: {} } })); await sleep();
    await fixtureClick(strip()!.querySelector("[data-slot=continue-now]")!);
    await until("retry error", () => !!strip()?.querySelector("[role=alert]")); seen.retryError = text(strip()!.querySelector("[role=alert]"));
    await fixtureClick(document.querySelector("[data-slot=model-selector-trigger]")!);
    await until("model picker", () => !!document.querySelector("[data-slot=model-picker-model]"));
    const sonnet = [...document.querySelectorAll("[data-slot=model-picker-model]")].find((el) => el.textContent?.includes("Sonnet"))!;
    await fixtureClick(sonnet); await until("same provider dispatch", () => asked.some((req) => req.method === "updateSetup"));
    // The cross-provider path reports queuing truthfully; it does not claim to have switched.
    await fixtureClick(document.querySelector("[data-slot=model-selector-trigger]")!);
    await until("picker reopened", () => !!document.querySelector("[data-slot=model-picker-agent]"));
    const codex = [...document.querySelectorAll("[data-slot=model-picker-agent]")].find((el) => el.textContent?.includes("Codex"))!;
    await fixtureClick(codex); await until("GPT row", () => [...document.querySelectorAll("[data-slot=model-picker-model]")].some((el) => el.textContent?.includes("GPT")));
    await fixtureClick([...document.querySelectorAll("[data-slot=model-picker-model]")].find((el) => el.textContent?.includes("GPT"))!);
    await until("cross provider dispatch", () => asked.some((req) => req.method === "continueOn")); await sleep();
    seen.pickerCalls = asked.filter((req) => req.method === "updateSetup" || req.method === "continueOn" || req.method === "sendMessage");
    seen.feedback = useToasts.getState().toasts.map((toast) => toast.text);
    // Don't resume removes the strip without sending any new text.
    await fixtureInput({ type: "key", key: "Escape" });
    await until("enabled cancel action", () => !strip()!.querySelector<HTMLButtonElement>("[data-slot=dont-resume]")!.disabled);
    strip()!.querySelector<HTMLButtonElement>("[data-slot=dont-resume]")!.click();
    await until("wait cancellation", () => !strip());
    seen.cancelled = !strip();
    const result = document.createElement("pre"); result.id = "quota-recovery-result"; result.textContent = JSON.stringify(seen); document.body.append(result);
  }
} catch (error) {
  const result = document.createElement("pre"); result.id = "quota-recovery-result";
  result.textContent = JSON.stringify({ error: String(error), asked, page: document.querySelector("main")?.textContent }); document.body.append(result);
}
