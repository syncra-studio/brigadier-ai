import { useEffect, useMemo } from "react";
import { useShallow } from "zustand/react/shallow";

import { openNotificationSettings, request } from "@/ipc/client";
import type { OvernightRun, TaskId } from "@/ipc/generated";
import { loadConversation, loadFullText, openConversation, switchBranch } from "@/state/actions";
import { type Board, updateBoard, useBoard } from "@/state/board";
import { setUpLidClosed, useKeepAwake } from "@/state/keepAwake";
import { loadNotificationPermission, useNotifications } from "@/state/notifications";
import { emptyThread, useApp } from "@/state/store";
import { toast } from "@/state/toasts";

/** Run facts translated for the shared card. */
export type OvernightDetails = {
  /** Run-scoped counts, never the whole conversation's counts. */
  waiting: number;
  decided: number;
  phaseProgress: Readonly<
    Record<
      string,
      {
        fixRound?: 1 | 2;
        quota?: { provider: string; resetsAtMs: number };
        workerTaskIds?: readonly TaskId[];
        criteria?: Readonly<Record<string, "verified" | "partial" | "blocked">>;
      }
    >
  >;
  /** Needed when a relative deadline has been resolved at Start. */
  reportReadyAtMs?: number;
  /** Exactly the report's three outcome lines, from recorded report facts. */
  outcome?: readonly [string, string, string];
  reportMessageId?: string;
  verifiedSha?: string;
  /** Use the conductor's remaining work, including intentionally skipped/deferred phases. */
  remainingPhaseIds: readonly string[];
  /** What Start can't hold, from the daemon's keep-awake status: a run keeps the computer
   * awake, screen on, and going with the lid closed whenever the lid can be held. */
  power?: { onBattery?: boolean; lidWillPause: boolean; offerLidSetup: boolean; lowBattery?: boolean };
  /** Notifications are off for Brigadier and this run's notification hasn't been shown. */
  notificationsOff?: boolean;
};

export type OvernightCardModel = {
  run: OvernightRun;
  details: OvernightDetails;
};

/** A command sees the revision/generation of the card the user actually acted on. */
export type OvernightCommand = {
  conversationId: string;
  runId: string;
  commandId: string;
  revision: number;
  generation: number;
};

export type OvernightActions = {
  start: (command: OvernightCommand) => Promise<unknown>;
  stop: (command: OvernightCommand) => Promise<unknown>;
  /** Creates a proposal on the same branch; it must never start work. */
  continue: (command: OvernightCommand, words: string) => Promise<unknown>;
  /** Run merge of the verified SHA, independent of the session's workspace. */
  merge: (command: OvernightCommand, verifiedSha: string) => Promise<unknown>;
  openReport: (conversationId: string, messageId: string) => void;
  setUpLidClosed: () => Promise<unknown>;
  openNotificationSettings: () => Promise<unknown>;
};

/** A continuation replaces only its own lineage, including bare goals without plan IDs. */
export function currentRuns(runs: Readonly<Record<string, OvernightRun>>): OvernightRun[] {
  const continued = new Set(Object.values(runs).flatMap((run) => run.predecessor ? [run.predecessor] : []));
  return Object.values(runs)
    .filter((run) => run.state !== "superseded" && !continued.has(run.id))
    .toSorted((a, b) => a.createdAtMs - b.createdAtMs);
}

export function projectOvernight(run: OvernightRun, board: Board, report?: string): OvernightCardModel {
  const belongs = (requestId: string | null) => requestId?.startsWith(`run-${run.id.slice(-8)}-`) ?? false;
  const ownsTask = (taskId: string) => board.tasks[taskId]?.run?.runId === run.id;
  const waiting = Object.values(board.waiting).filter((item) => {
    const source = item.source;
    if (source.type === "run") return source.runId === run.id;
    if (source.type === "task" || source.type === "landing") return ownsTask(source.taskId);
    return belongs(item.requestId);
  }).length;
  const decided = board.decisions.filter((item) => {
    if (item.source.type === "run") return item.source.runId === run.id;
    if (item.source.type === "task") return ownsTask(item.source.taskId);
    return belongs(item.requestId);
  }).length;
  const phaseProgress: OvernightDetails["phaseProgress"] = Object.fromEntries(run.phases.map((phase) => {
    // Verified phases retained by Continue keep their original request and task ownership.
    const tasks = Object.values(board.tasks).filter((task) =>
      task.run?.phaseId === phase.id &&
      ((task.run.runId === run.id && task.run.generation === run.generation) ||
        (phase.state === "verified" && phase.requestId !== null && task.requestId === phase.requestId)),
    ).toSorted((a, b) => a.number - b.number);
    const quotaTask = tasks.filter((task) => task.quotaWait?.resetsAtMs != null)
      .toSorted((a, b) => a.quotaWait!.resetsAtMs! - b.quotaWait!.resetsAtMs!)[0];
    return [phase.id, {
      ...(phase.fixRounds > 0 && phase.state === "running" ? { fixRound: Math.min(2, phase.fixRounds) as 1 | 2 } : {}),
      ...(quotaTask ? { quota: {
        provider: quotaTask.route.choice.provider === "claude" ? "Claude" : "Codex",
        resetsAtMs: quotaTask.quotaWait!.resetsAtMs!,
      } } : {}),
      workerTaskIds: tasks.map((task) => task.id),
      criteria: Object.fromEntries(phase.criteria.map((criterion) => [criterion.id,
        criterion.status === "met" ? "verified" : criterion.status === "blocked" ? "blocked" : "partial",
      ])),
    }];
  }));
  // The report renderer owns these three paragraphs; never infer success in the frontend.
  const lines = (run.reportOutcome ?? report?.split(/\n\s*\n/).slice(0, 3))
    ?.map((line) => line.replace(/[*`]/g, "").trim());
  return { run, details: {
    waiting, decided, phaseProgress,
    ...(lines?.length === 3 ? { outcome: lines as [string, string, string] } : {}),
    ...(run.reportMessageId ? { reportMessageId: run.reportMessageId } : {}),
    ...(run.verifiedCommit && run.verifiedCommit !== run.workspace?.baseCommit ? { verifiedSha: run.verifiedCommit } : {}),
    remainingPhaseIds: run.phases.length === 0 ? ["phase-0"] :
      run.phases.filter((phase) => phase.state !== "verified").map((phase) => phase.id),
  } };
}

export function useOvernightCards(conversationId: string): readonly OvernightCardModel[] {
  const board = useBoard((s) => s.board?.conversationId === conversationId ? s.board : null);
  const thread = useApp((s) => s.threads[conversationId] ?? emptyThread);
  const status = useKeepAwake((s) => s.status);
  const leadQuota = useApp((s) => s.conversations[conversationId]?.quotaWait);
  const runs = useMemo(() => currentRuns(board?.overnight ?? {}), [board?.overnight]);
  const notifications = useNotifications((s) => s.permission);
  const hasRuns = runs.length > 0;
  useEffect(() => {
    if (!hasRuns) return;
    // Asking the OS doesn't prompt; checked again while the card can be seen, so turning
    // notifications on in System Settings (or answering Start's prompt) shows up here.
    const load = () => {
      if (!useApp.getState().windowVisible) return;
      loadNotificationPermission().catch((error: unknown) => console.error("checking notifications failed", error));
    };
    load();
    const timer = window.setInterval(load, 10_000);
    window.addEventListener("focus", load);
    return () => {
      window.clearInterval(timer);
      window.removeEventListener("focus", load);
    };
  }, [hasRuns]);
  const reports = useApp(useShallow((s) => runs.map((run) => {
    const message = s.threads[conversationId]?.items.find((item) => item.id === run.reportMessageId);
    return message ? s.threads[conversationId]?.fullText[message.id] ?? message.text : undefined;
  })));
  useEffect(() => {
    for (const run of runs) {
      const message = thread.items.find((item) => item.id === run.reportMessageId);
      if (message?.blob && !thread.fullText[message.id])
        void loadFullText(conversationId, message.id, message.blob).catch((error: unknown) => console.error(error));
    }
  }, [conversationId, runs, thread]);
  return useMemo(() => board ? runs.map((run, index) => {
    const model = projectOvernight(run, board, reports[index]);
    const waitsForNotice = run.state === "finished"
      ? run.notification != null && run.notification.deliveredAtMs == null
      : run.state !== "superseded";
    if (notifications === "off" && waitsForNotice) model.details.notificationsOff = true;
    if (status && run.state === "proposed") model.details.power = {
      onBattery: status.onBattery,
      lidWillPause: status.lidClosed !== "active" && status.lidClosed !== "ready",
      offerLidSetup: status.lidClosed === "needsSetup",
      lowBattery: status.lidClosed === "lowBattery",
    };
    const active = run.phases.find((phase) => phase.state === "running" || phase.state === "checking");
    if (run.state === "waitingQuota" && active && leadQuota?.resetsAtMs) {
      model.details.phaseProgress = { ...model.details.phaseProgress, [active.id]: {
        ...model.details.phaseProgress[active.id], quota: {
          provider: active.lead?.provider === "claude" ? "Claude" : "Codex", resetsAtMs: leadQuota.resetsAtMs,
        },
      } };
    }
    return model;
  }) : [], [board, runs, reports, status, leadQuota, notifications]);
}

function seen(command: OvernightCommand): OvernightRun {
  const current = useBoard.getState().board?.overnight[command.runId];
  if (!current || current.revision !== command.revision || current.generation !== command.generation)
    throw new Error("The run changed. Check the card and try again.");
  return current;
}

function received(run: OvernightRun, before: OvernightRun): OvernightRun {
  // Live events may have arrived before the IPC response. Never replace a newer snapshot.
  updateBoard(run.conversationId, (board) => {
    const current = board.overnight[run.id];
    return current && current !== before ? board : { ...board, overnight: { ...board.overnight, [run.id]: run } };
  });
  return run;
}

export const overnightActions: OvernightActions = {
  async start(command) {
    const before = seen(command);
    const { run } = await request({ method: "startOvernight", conversationId: command.conversationId,
      runId: command.runId, commandId: command.commandId, revision: command.revision });
    return received(run, before);
  },
  async stop(command) {
    const before = seen(command);
    const { run } = await request({ method: "stopOvernight", conversationId: command.conversationId,
      runId: command.runId, commandId: command.commandId });
    return received(run, before);
  },
  async continue(command, words) {
    const before = seen(command);
    const { run } = await request({ method: "continueOvernight", conversationId: command.conversationId,
      runId: command.runId, commandId: command.commandId, words });
    return received(run, before);
  },
  async merge(command, verifiedSha) {
    const before = seen(command);
    const { run } = await request({ method: "mergeOvernight", conversationId: command.conversationId,
      runId: command.runId, commandId: command.commandId, verifiedCommit: verifiedSha });
    return received(run, before);
  },
  openReport(conversationId, messageId) {
    void (async () => {
      const selection = useApp.getState().selection;
      if (selection.type !== "conversation" || selection.id !== conversationId) openConversation(conversationId);
      await switchBranch(conversationId, messageId);
      await loadConversation(conversationId);
      requestAnimationFrame(() => requestAnimationFrame(() => {
        const message = document.getElementById(`message-${messageId}`);
        message?.scrollIntoView({ block: "center" });
        message?.focus({ preventScroll: true });
      }));
    })().catch((error: unknown) => toast(String(error), { tone: "error" }));
  },
  // Only for the runs: the saved lid setting stays as it is.
  setUpLidClosed,
  openNotificationSettings,
};

export function overnightCommand(run: OvernightRun): OvernightCommand {
  return {
    conversationId: run.conversationId,
    runId: run.id,
    commandId: crypto.randomUUID(),
    revision: run.revision,
    generation: run.generation,
  };
}

export function deadlineLabel(model: OvernightCardModel): string {
  const { run, details } = model;
  const deadline = run.directives.deadline;
  if (deadline.type === "untilDone") return "until done";
  if (deadline.type === "at") return `until ${deadline.time.localTime}`;
  if (details.reportReadyAtMs !== undefined) {
    return `until ${new Date(details.reportReadyAtMs).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", hour12: false })}`;
  }
  // A duration on an unstarted proposal has no fixed wall-clock deadline yet.
  const hours = deadline.minutes / 60;
  if (deadline.minutes % 60 === 0) return `for ${hours} ${hours === 1 ? "hour" : "hours"}`;
  return `for ${deadline.minutes} ${deadline.minutes === 1 ? "minute" : "minutes"}`;
}

export function restrictionLines(run: OvernightRun): string[] {
  const { only, stopAfter, skip, maxWorkers } = run.directives;
  return [
    ...(only ? [`Only phases ${only.from}–${only.to}`] : []),
    ...(stopAfter
      ? [
          stopAfter.type === "phase"
            ? `Stop after phase ${stopAfter.number}`
            : "Stop after this phase",
        ]
      : []),
    ...(skip.length
      ? [`Skip ${skip.length === 1 ? "phase" : "phases"} ${skip.join(", ")}`]
      : []),
    ...(maxWorkers !== null
      ? [
          `At most ${maxWorkers} ${maxWorkers === 1 ? "worker" : "workers"} at once`,
        ]
      : []),
  ];
}
