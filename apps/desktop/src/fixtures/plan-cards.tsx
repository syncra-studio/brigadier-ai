/** A standalone Vite entry, excluded from the app entry and production build. Synthetic data. */
import {
  AssistantRuntimeProvider,
  ComposerPrimitive,
  useExternalStoreRuntime,
  type ThreadMessage,
} from "@/components/conversation";
import { mockIPC } from "@tauri-apps/api/mocks";
import { useState } from "react";
import { createRoot } from "react-dom/client";

import { PlanCardLink } from "@/app/conversation/cards/PlanCardLink";
import { PendingActionCard } from "@/app/conversation/ActionCards";
import { PinnedSummary, PinnedSummaryToggle, SummaryFloat, SummaryPane } from "@/app/conversation/PinnedSummary";
import { SlashCommands } from "@/app/conversation/SlashCommands";
import { AgentsPanelContext } from "@/app/conversation/WorkerChip";
import { TooltipProvider } from "@/components/overlays";
import type {
  Conversation,
  Plan,
  PlanState,
  Request,
  Task,
  Setup,
} from "@/ipc/generated";
import { emptyBoard, useBoard } from "@/state/board";
import { useApp } from "@/state/store";

const now = new Date("2026-10-02T23:00:00+03:00").getTime();
const session: Conversation = {
  id: "fixture-session",
  kind: "session",
  projectId: null,
  title: "Plan fixtures",
  pinnedAtMs: null,
  createdAtMs: now,
  updatedAtMs: now,
  lifecycle: "active",
  forkedFrom: null,
  sideOf: null,
  fallback: null,
  quotaWait: null,
  setup: {
    type: "session",
    repo: "/tmp/plan-card-fixture",
    environment: { type: "localCheckout", branch: "fixture" },
    permission: "ask",
    planMode: false,
    workersSeeUncommitted: null,
    orchestrator: { provider: "claude", model: null, effort: null },
  },
};
const automatic: Conversation = {
  ...session,
  id: "fixture-automatic",
  setup: { ...(session.setup as Extract<Setup, { type: "session" }>), permission: "acceptEdits" },
};

const states: { id: string; name: string; state: PlanState; automatic?: boolean }[] = [
  { id: "normal", name: "Normal · waiting for approval", state: { type: "proposed" } },
  {
    id: "automatic",
    name: "Normal · Brigadier decides",
    state: { type: "proposed" },
    automatic: true,
  },
  {
    id: "approved-user",
    name: "Normal · approved by you",
    state: { type: "approved", by: "user" },
  },
  {
    id: "approved-auto",
    name: "Normal · auto-approved",
    state: { type: "approved", by: "orchestrator" },
    automatic: true,
  },
  {
    id: "rejected",
    name: "Normal · rejected",
    state: { type: "rejected", message: "Keep the existing command names." },
  },
];
/** A phase that has not started. */
const idle = { stage: "pending", startedAtMs: null, endedAtMs: null, outline: null } as const;
const plans: Plan[] = states.map((item, index) => ({
  id: item.id,
  conversationId: item.automatic ? automatic.id : session.id,
  requestId: null,
  position: index,
  title: "Windows support",
  body: "# Windows support\n\nSupport Windows alongside macOS.",
  state: item.state,
  createdAtMs: now,
  decidedAtMs: null,
  steps: [
    {
      title: "Read the platform code",
      detail: "Find the path and terminal differences.",
      taskId: "worker-done",
      ...idle,
    },
    {
      title: "Implement Windows path handling without changing the existing macOS behavior",
      detail: "Check both path separators and drive letters.",
      taskId: "worker-live",
      ...idle,
    },
    { title: "Verify the build", detail: "Run the platform checks.", taskId: null, ...idle },
  ],
}));
const old: Plan = {
  ...plans[0]!,
  id: "older",
  position: -1,
  title: "Windows support · earlier plan",
  state: { type: "superseded" },
};
const board = emptyBoard(session.id);
board.plans = Object.fromEntries([...plans, old].map((plan) => [plan.id, plan]));
// Only the fields WorkerChip and the context card's Workers row read are needed by this
// synthetic board.
board.tasks = Object.fromEntries(
  [
    {
      id: "worker-done",
      number: 1,
      title: "Read platform code",
      state: "done",
      kind: "read",
      quotaWait: null,
      attempts: [],
      createdAtMs: now,
      updatedAtMs: now,
    },
    {
      id: "worker-live",
      number: 2,
      title: "Implement Windows paths",
      state: "running",
      kind: "write",
      quotaWait: null,
      attempts: [],
      createdAtMs: now,
      updatedAtMs: now,
    },
  ].map((task) => [task.id, task as unknown as Task]),
);
useBoard.setState({ board });
useApp.setState({
  conversations: { [session.id]: session, [automatic.id]: automatic },
  selection: { type: "conversation", id: session.id },
  summaryShown: { [session.id]: true },
});

const calls: unknown[] = [];
const fixtureWindow = window as typeof window & {
  planFixtureCalls: unknown[];
  planFixtureFailNext: boolean;
};
fixtureWindow.planFixtureCalls = calls;
fixtureWindow.planFixtureFailNext = false;
mockIPC((command, payload) => {
  if (command !== "ipc_request") throw new Error(`Unexpected fixture command: ${command}`);
  const req = (payload as { request: Request }).request;
  calls.push(req);
  if (fixtureWindow.planFixtureFailNext) {
    fixtureWindow.planFixtureFailNext = false;
    throw new Error("Connection lost. Try again.");
  }
  if (req.method === "getPullRequest") return { method: req.method, pullRequest: null };
  if (req.method === "decidePlan") {
    const current = useBoard.getState().board;
    const plan = current?.plans[req.cardId];
    if (current && plan)
      useBoard.setState({
        board: {
          ...current,
          plans: {
            ...current.plans,
            [plan.id]: {
              ...plan,
              state: req.approve
                ? { type: "approved", by: "user" }
                : { type: "rejected", message: req.message },
            },
          },
        },
      });
    return { method: req.method };
  }
  throw new Error(`Unexpected fixture request: ${req.method}`);
});

function FixturePage() {
  const [worker, setWorker] = useState<string | null | undefined>(undefined);
  const runtime = useExternalStoreRuntime<ThreadMessage>({
    messages: [],
    onNew: async () => {
      calls.push({ method: "sendMessage" });
    },
  });
  const summary = new URLSearchParams(location.search).has("summary");
  const rail = new URLSearchParams(location.search).has("rail");
  return (
    <AssistantRuntimeProvider runtime={runtime}>
      <TooltipProvider>
        <AgentsPanelContext.Provider value={{ panel: worker, setPanel: setWorker }}>
          <main className="mx-auto flex min-h-full max-w-screen-xl flex-col gap-6 p-4">
            <div className="flex flex-wrap items-center gap-3">
              <h1 className="text-lg">Plan card fixtures</h1>
              <span className="text-muted-foreground text-xs">
                Synthetic data · no daemon calls
              </span>
            </div>
            {worker && <p role="status">Worker thread opened: {worker}</p>}
            {rail && (
              <PendingActionCard
                action={{ type: "plan", id: "normal" }}
                more={0}
                onDismiss={() => calls.push({ method: "dismissPlan" })}
              />
            )}
            {summary ? (
              <SummaryFloat>
                <div className="flex justify-end">
                  <PinnedSummaryToggle />
                </div>
                <SummaryPane summary>
                  <div className="flex h-screen flex-col gap-4 p-4">
                    <PlanCardLink cardId="normal" />
                    <PlanCardLink cardId="older" />
                  </div>
                  <PinnedSummary conversation={session} />
                </SummaryPane>
              </SummaryFloat>
            ) : (
              <div className="grid grid-cols-1 items-start gap-6 md:grid-cols-2 xl:grid-cols-3">
                {states.map((item) => (
                  <section
                    key={item.id}
                    data-fixture={item.id}
                    className="flex min-w-0 flex-col gap-2"
                  >
                    <h2 className="text-muted-foreground text-sm">{item.name}</h2>
                    {/* As the thread shows it: the context card has no plan section (Q88). */}
                    <PlanCardLink cardId={item.id} />
                  </section>
                ))}
              </div>
            )}
            <div className="relative" data-fixture="slash">
              <ComposerPrimitive.Unstable_TriggerPopoverRoot>
                <SlashCommands conversation={session} groups={[]} onOpenModel={() => {}} />
                <ComposerPrimitive.Root>
                  <ComposerPrimitive.Input
                    aria-label="Fixture message"
                    placeholder="Type /overnight"
                    className="bg-muted rounded-control w-full p-3"
                  />
                </ComposerPrimitive.Root>
              </ComposerPrimitive.Unstable_TriggerPopoverRoot>
            </div>
          </main>
        </AgentsPanelContext.Provider>
      </TooltipProvider>
    </AssistantRuntimeProvider>
  );
}

createRoot(document.getElementById("root")!).render(<FixturePage />);

// For PlanCardsRender.test.ts: the plans' rows the page drew once it settled, and any plan
// section left in a context card.
setTimeout(() => {
  const result = document.createElement("pre");
  result.id = "plan-cards-result";
  result.textContent = JSON.stringify({
    rows: [...document.querySelectorAll<HTMLElement>("[data-fixture]")].filter((section) => section.querySelector("[data-slot=plan-row]")).map((section) => section.dataset["fixture"]),
    inContext: document.querySelectorAll("[aria-label=Context] [data-card=plan]").length,
  });
  document.body.append(result);
}, 1000);
