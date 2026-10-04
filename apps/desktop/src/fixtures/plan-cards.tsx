/** A standalone Vite entry, excluded from the app entry and production build. Synthetic data. */
import {
  AssistantRuntimeProvider,
  ComposerPrimitive,
  useExternalStoreRuntime,
  type ThreadMessage,
} from "@assistant-ui/react";
import { mockIPC } from "@tauri-apps/api/mocks";
import { useState } from "react";
import { createRoot } from "react-dom/client";

import { OvernightPlanCard } from "@/app/conversation/cards/OvernightPlanCard";
import { PlanSection } from "@/app/conversation/cards/PlanSection";
import { PlanCardLink } from "@/app/conversation/cards/PlanCardLink";
import { PendingActionCard } from "@/app/conversation/ActionCards";
import { PinnedSummary, PinnedSummaryToggle, SummaryFloat, SummaryPane } from "@/app/conversation/PinnedSummary";
import { SlashCommands } from "@/app/conversation/SlashCommands";
import { AgentsPanelContext } from "@/app/conversation/WorkerChip";
import {
  useOvernightCards,
  type OvernightActions,
  type OvernightCardModel,
  type OvernightCommand,
} from "@/app/conversation/overnightAdapter";
import { Button } from "@/components/ui/button";
import { TooltipProvider } from "@/components/ui/tooltip";
import type {
  Conversation,
  OvernightRun,
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
    permission: "askForApproval",
    planMode: false,
    workersSeeUncommitted: null,
    orchestrator: { provider: "claude", model: null, effort: null },
  },
};
const automatic: Conversation = {
  ...session,
  id: "fixture-automatic",
  setup: { ...(session.setup as Extract<Setup, { type: "session" }>), permission: "approveForMe" },
};

const states: { id: string; name: string; state: PlanState; automatic?: boolean }[] = [
  { id: "normal", name: "Normal · waiting for approval", state: { type: "proposed" } },
  {
    id: "automatic",
    name: "Normal · Brigadier decides",
    state: { type: "proposed" },
    automatic: true,
  },
  { id: "review", name: "Normal · in review", state: { type: "inReview", taskId: "reviewer" } },
  { id: "revising", name: "Normal · being revised", state: { type: "revising" } },
  {
    id: "approved-user",
    name: "Normal · approved by you",
    state: { type: "approved", by: "user" },
  },
  {
    id: "approved-auto",
    name: "Normal · approved after review",
    state: { type: "approved", by: "review" },
    automatic: true,
  },
  {
    id: "approved-brigadier",
    name: "Normal · auto-approved",
    state: { type: "approved", by: "brigadier" },
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
  risky: false,
  state: item.state,
  gate:
    item.id === "review"
      ? {
          rebased: false,
          verificationScope: { type: "full", reason: "Plan review" },
          round: 1,
          commit: null,
          members: [],
          outcome: null,
          relanding: false,
          retry: false,
          overridden: false,
          findings: [{ id: "F1", text: "Check the path handling on Windows.", by: "reviewer" }],
        }
      : null,
  revises: item.id === "revising" ? "older" : null,
  responses: [],
  reviewNotes: [],
  reviewSkipReason: null,
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
  title: "Windows support · earlier revision",
  state: { type: "superseded" },
};
const board = emptyBoard(session.id);
board.plans = Object.fromEntries([...plans, old].map((plan) => [plan.id, plan]));
// Only WorkerChip's fields are needed by this synthetic board.
board.tasks = Object.fromEntries(
  [
    {
      id: "worker-done",
      number: 1,
      title: "Read platform code",
      state: "done",
      kind: "read",
      quotaWait: null,
    },
    {
      id: "worker-live",
      number: 2,
      title: "Implement Windows paths",
      state: "running",
      kind: "write",
      quotaWait: null,
    },
  ].map((task) => [task.id, task as Task]),
);
useBoard.setState({ board });
useApp.setState({
  conversations: { [session.id]: session, [automatic.id]: automatic },
  pinnedSummary: true,
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

// Keep the active phase's link distinct from the normal plan's Windows-path worker. None of the
// synthetic workers is a check.
const fixtureBoard = useBoard.getState().board!;
useBoard.setState({ board: { ...fixtureBoard, tasks: Object.fromEntries(Object.entries({
  ...fixtureBoard.tasks,
  "worker-phase-5": { ...fixtureBoard.tasks["worker-live"]!, id: "worker-phase-5",
    title: "Review the whole Windows change", number: 5 },
}).map(([taskId, task]) => [taskId, { ...task, gateLink: task.gateLink ?? null }])) } });

const proposed: OvernightRun = {
  id: "proposed",
  conversationId: session.id,
  segment: 1,
  predecessor: null,
  planId: null,
  name: "Windows support",
  words: "/overnight Windows support until 07:30",
  goal: "Support Windows alongside macOS.",
  rules: "Keep existing commands. Don't change the installer.",
  sources: [],
  phases: Array.from({ length: 6 }, (_, index) => ({
    id: `phase-${index + 1}`,
    number: index + 1,
    name: [
      "Read the platform code",
      "Implement Windows paths",
      "Verify the terminal",
      "Build the desktop app",
      "Review the whole change and check the Windows installer against the existing macOS behavior",
      "Write the report",
    ][index]!,
    scope: "Keep the existing workflow and verify the platform differences.",
    doneWhen: [
      { id: `p${index + 1}-c1`, text: "The existing commands still work on both platforms." },
      {
        id: `p${index + 1}-c2`,
        text: "The phase checks pass and the independent review is answered.",
      },
    ],
    dependsOn: index ? [index] : [],
    state: "pending",
    requestId: null,
    startCommit: null,
    verifiedCommit: null,
    gate: null,
    fixRounds: 0,
    criteria: [],
    gaps: [],
    summary: null,
    responses: [],
    lead: null,
    nudges: 0,
    startedAtMs: null,
    settledAtMs: null,
  })),
  directives: {
    deadline: {
      type: "at",
      time: {
        atMs: now + 8.5 * 3600000,
        localDate: "2026-10-03",
        localTime: "07:30",
        day: "Sat 3 Oct",
        offset: "+03:00",
        timeZone: "Europe/Chisinau",
      },
    },
    only: { from: 1, to: 6 },
    stopAfter: { type: "phase", number: 5 },
    skip: [3],
    maxWorkers: 2,
    ignored: ["Ignored: effort max (Brigadier picks this itself)"],
    spans: [],
  },
  problems: [],
  revision: 2,
  generation: 1,
  state: "proposed",
  windDownAtMs: null,
  workspace: null,
  planning: null,
  verifiedCommit: null,
  gaps: [],
  obstacles: [],
  reportMessageId: null,
  reportOutcome: null,
  reportVersion: 0,
  reportText: null,
  endCommit: null,
  merged: null,
  notification: null,
  stop: null,
  commands: [],
  createdAtMs: now,
  startedAtMs: null,
  finishedAtMs: null,
};
const baseline: OvernightCardModel = {
  run: proposed,
  details: { waiting: 1, decided: 3, phaseProgress: {}, remainingPhaseIds: [] },
};
function variant(
  id: string,
  state: OvernightRun["state"],
  progress: OvernightCardModel["details"]["phaseProgress"] = {},
): OvernightCardModel {
  return {
    run: {
      ...proposed,
      id,
      state,
      phases: proposed.phases.map((phase, index) => ({
        ...phase,
        state: index < 4 ? "verified" : index === 4 ? "running" : "pending",
      })),
    },
    details: { ...baseline.details, phaseProgress: progress },
  };
}
const overnight: { label: string; model: OvernightCardModel }[] = [
  { label: "Overnight · proposed", model: baseline },
  {
    label: "Bare goal",
    model: {
      ...baseline,
      run: {
        ...proposed,
        id: "bare-goal",
        phases: [],
        directives: {
          ...proposed.directives,
          deadline: { type: "untilDone" },
          only: null,
          stopAfter: null,
          skip: [],
        },
      },
    },
  },
  {
    label: "Proposal · power risks",
    model: {
      ...baseline,
      run: { ...proposed, id: "power" },
      details: {
        ...baseline.details,
        power: { onBattery: true, lidWillPause: true, offerLidSetup: true },
      },
    },
  },
  {
    label: "Proposal · invalid deadline",
    model: {
      ...baseline,
      run: {
        ...proposed,
        id: "invalid",
        problems: [
          {
            kind: "deadline",
            message: "That deadline has passed. Say when the report should be ready.",
          },
        ],
      },
    },
  },
  { label: "Preparing", model: variant("preparing", "preparing") },
  {
    label: "Writing the plan",
    model: {
      ...variant("planning", "planning"),
      run: { ...proposed, id: "planning", state: "planning", phases: [] },
    },
  },
  {
    label: "Running · lead working",
    model: variant("running", "running", { "phase-5": { workerTaskIds: ["worker-phase-5"] } }),
  },
  {
    label: "Running · checking",
    model: {
      ...variant("checking", "phaseGate"),
      run: {
        ...variant("checking", "phaseGate").run,
        phases: proposed.phases.slice(0, 2).map((phase) => ({ ...phase, state: "checking" })),
      },
    },
  },
  {
    label: "Running · fixing 1/2",
    model: variant("fixing-1", "running", { "phase-5": { fixRound: 1 } }),
  },
  {
    label: "Running · fixing 2/2",
    model: variant("fixing-2", "running", { "phase-5": { fixRound: 2 } }),
  },
  {
    label: "Waiting for limits",
    model: variant("quota", "waitingQuota", {
      "phase-5": {
        quota: { provider: "Claude", resetsAtMs: new Date("2026-10-03T03:40:00+03:00").getTime() },
      },
    }),
  },
  { label: "Winding down", model: variant("winding-down", "windingDown") },
  { label: "Writing the report", model: variant("reporting", "reporting") },
  {
    label: "Finished · partial",
    model: {
      run: {
        ...proposed,
        id: "partial",
        state: "finished",
        phases: proposed.phases.map((phase, index) => ({
          ...phase,
          state: (["verified", "partial", "blocked", "skipped", "verified", "pending"] as const)[
            index
          ]!,
        })),
      },
      details: {
        ...baseline.details,
        outcome: [
          "2 of 6 phases verified",
          "Windows paths are ready; the installer needs your account ID.",
          "1 waiting on you · work saved on its own branch",
        ],
        reportMessageId: "report-partial",
        verifiedSha: "abc1234",
        remainingPhaseIds: ["phase-2", "phase-3", "phase-6"],
      },
    },
  },
  {
    label: "Finished · all verified",
    model: {
      run: {
        ...proposed,
        id: "complete",
        state: "finished",
        phases: proposed.phases.map((phase) => ({ ...phase, state: "verified" })),
      },
      details: {
        ...baseline.details,
        waiting: 0,
        outcome: [
          "6 of 6 phases verified",
          "Windows support is ready to merge.",
          "All checks passed · work saved on its own branch",
        ],
        reportMessageId: "report-complete",
        verifiedSha: "def5678",
      },
    },
  },
];

async function record(method: string, command?: OvernightCommand, extra?: unknown) {
  calls.push({ method, ...command, extra });
  await new Promise((resolve) => setTimeout(resolve, 60));
  if (fixtureWindow.planFixtureFailNext) {
    fixtureWindow.planFixtureFailNext = false;
    throw new Error("Connection lost. Try again.");
  }
}

function OvernightFixture({ initial }: { initial: OvernightCardModel }) {
  const [model, setModel] = useState(initial);
  const actions: OvernightActions = {
    start: async (command) => {
      await record("startOvernight", command);
      setModel({
        ...model,
        run: {
          ...model.run,
          state: model.run.phases.length ? "running" : "planning",
          phases: model.run.phases.map((phase, index) => ({
            ...phase,
            state: index ? "pending" : "running",
          })),
        },
      });
    },
    stop: (command) => record("stopOvernight", command),
    continue: async (command, words) => {
      await record("continueOvernight", command, words);
      setModel({
        ...model,
        run: {
          ...model.run,
          state: "proposed",
          phases: model.run.phases.filter((phase) =>
            model.details.remainingPhaseIds.includes(phase.id),
          ),
          directives: { ...model.run.directives, deadline: { type: "untilDone" } },
        },
      });
    },
    merge: (command, sha) => record("mergeOvernight", command, sha),
    openReport: (_conversationId, messageId) => {
      calls.push({ method: "openReport", messageId });
      document.getElementById("fixture-report")?.focus();
    },
    setUpLidClosed: () => record("setUpLidClosed"),
    openNotificationSettings: () => record("openNotificationSettings"),
  };
  return <OvernightPlanCard model={model} actions={actions} />;
}

function FixturePage() {
  const [compact, setCompact] = useState(false);
  const [worker, setWorker] = useState<string | null | undefined>(undefined);
  const cards = useOvernightCards(session.id);
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
              <Button
                type="button"
                size="sm"
                variant="outline"
                onClick={() => {
                  const next = !compact;
                  setCompact(next);
                  document.documentElement.dataset.density = next ? "compact" : "normal";
                }}
              >
                {compact ? "Normal density" : "Compact density"}
              </Button>
              <span className="text-muted-foreground text-xs">
                Synthetic data · no daemon calls · {cards.length} reachable overnight cards
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
                    {/* As in the summary's context card. */}
                    <div className="bg-popover rounded-summary shadow-summary py-2.5">
                      <PlanSection planIds={[item.id]} />
                    </div>
                  </section>
                ))}
                {overnight.map(({ label, model }) => (
                  <section
                    key={model.run.id}
                    data-fixture={model.run.id}
                    className="flex min-w-0 flex-col gap-2"
                  >
                    <h2 className="text-muted-foreground text-sm">{label}</h2>
                    <OvernightFixture initial={model} />
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
            <p id="fixture-report" tabIndex={-1}>
              Fixture report message: work is saved; no live run was started.
            </p>
          </main>
        </AgentsPanelContext.Provider>
      </TooltipProvider>
    </AssistantRuntimeProvider>
  );
}

createRoot(document.getElementById("root")!).render(<FixturePage />);
