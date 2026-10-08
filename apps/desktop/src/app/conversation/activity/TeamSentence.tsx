import { CheckCircle, Chat, Commit, StopCircle } from "@openai/apps-sdk-ui/components/Icon";
import { type FC, type ReactNode, useContext } from "react";
import { useShallow } from "zustand/react/shallow";

import { type BlockRow, lifecycleWord } from "@/app/conversation/blocks";
import { AgentsPanelContext, useWorkerName, WorkerGlyph } from "@/app/conversation/WorkerChip";
import { workerState } from "@/app/conversation/workerPresentation";
import { ROW, ROW_DETAIL } from "@/components/assistant-ui/elements/activity-row";
import { ThreadActivity } from "@/components/assistant-ui/elements/thread-activity";
import type { OrchestratorStepKind, Task } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { useBoard } from "@/state/board";

/**
 * How workers show in the thread (THREAD-UX-PLAN.md §3.4): sentences in the row recipe of the
 * work, never cards. What happened to them ("Daemon storage and Thread rows finished") and what
 * the lead did to them ("Messaged Thread rows", "Stopped Fix uploads"); every name opens its
 * worker.
 */

/**
 * A worker's name as a button that opens its thread, with its glyph unless the sentence shows
 * the glyphs up front. The one way a worker is named in the thread, the live line and the strip.
 */
export const WorkerName: FC<{ taskId: string; glyph?: boolean }> = ({ taskId, glyph = true }) => {
  const { setPanel } = useContext(AgentsPanelContext);
  const name = useWorkerName(taskId);
  if (name === null) return <span className="shrink-0">a worker</span>;
  return (
    <button
      type="button"
      data-slot="worker-name"
      onClick={(event) => {
        event.stopPropagation();
        setPanel(taskId);
      }}
      title={`Open ${name}`}
      className="text-foreground/90 hover:text-foreground rounded-control inline-flex max-w-full min-w-0 items-center gap-1 align-bottom"
    >
      {glyph && <WorkerGlyph taskId={taskId} className="size-icon-xs shrink-0" />}
      <span className="min-w-0 truncate">{name}</span>
    </button>
  );
};

/** "A", "A and B", "A, B and C"; past three, "A, B and 4 more", the more opening the Workers list. */
export const Names: FC<{ taskIds: readonly string[]; glyph?: boolean }> = ({ taskIds, glyph = false }) => {
  const { setPanel } = useContext(AgentsPanelContext);
  const named = taskIds.length > 3 ? taskIds.slice(0, 2) : taskIds;
  const more = taskIds.length - named.length;
  const parts: ReactNode[] = named.map((id) => <WorkerName key={id} taskId={id} glyph={glyph} />);
  if (more > 0) {
    parts.push(
      <button key="more" type="button" className="hover:text-foreground rounded-control" onClick={() => setPanel(null)}>
        {more} more
      </button>,
    );
  }
  return (
    <>
      {parts.map((part, index) => (
        <span key={index} className="min-w-0">
          {index > 0 && (index === parts.length - 1 ? " and " : ", ")}
          {part}
        </span>
      ))}
    </>
  );
};

/** The glyphs of a sentence's workers, up front: one each, the first four. */
const Glyphs: FC<{ taskIds: readonly string[] }> = ({ taskIds }) => (
  <span aria-hidden className="inline-flex h-5 shrink-0 items-center gap-1.5">
    {taskIds.slice(0, 4).map((id) => (
      <WorkerGlyph key={id} taskId={id} className="size-4" />
    ))}
  </span>
);

/** A finished report that names a gap: its checks failed, or its review asked for changes. */
function withProblems(task: Task | undefined): boolean {
  return task?.report?.checks === "failed" || task?.report?.verdict === "requestChanges";
}

function verb(word: ReturnType<typeof lifecycleWord>, tasks: readonly (Task | undefined)[]): string {
  const many = tasks.length > 1;
  switch (word) {
    case "started":
      return "started working";
    case "waiting":
      return many ? "are waiting" : "is waiting for an answer";
    case "stopped":
      return "stopped";
    case "failed":
      return "failed";
    case "finished":
      return tasks.every(withProblems) ? "finished with problems" : "finished";
  }
}

/** A start's detail: each worker's brief, in a quiet box. */
const Briefs: FC<{ tasks: readonly Task[] }> = ({ tasks }) => (
  <div className="flex flex-col gap-2">
    {tasks.map((task) => (
      <div key={task.id} className="flex flex-col gap-1">
        {tasks.length > 1 && <WorkerName taskId={task.id} />}
        <p className="bg-code-surface rounded-control text-foreground/80 max-h-60 overflow-y-auto px-3 py-2 whitespace-pre-wrap">
          {task.spec}
        </p>
      </div>
    ))}
  </div>
);

/** What happened to workers, as one sentence: "Daemon storage, Thread rows and Thread phases started working". */
export const TeamSentence: FC<{ row: BlockRow }> = ({ row }) => {
  const ids = row.taskIds ?? [row.taskId];
  const tasks = useBoard(useShallow((s) => ids.map((id) => s.board?.tasks[id])));
  const known = tasks.filter((task): task is Task => !!task);
  if (known.length === 0) return null;
  const word = row.kind === undefined && known[0] ? lifecycleWordOf(known[0]) : lifecycleWord(row.kind);
  return (
    <ThreadActivity
      data-slot="task-row"
      data-kind={word}
      className={cn(ROW, "items-start")}
      detail={word === "started" ? <Briefs tasks={known} /> : undefined}
      detailClassName={cn(ROW_DETAIL, "max-h-none")}
    >
      <Glyphs taskIds={ids} />
      <span className="min-w-0">
        <Names taskIds={ids} /> {verb(word, tasks)}
      </span>
    </ThreadActivity>
  );
};

/** An old task's sentence, from its state when its board has no lifecycle event for it. */
function lifecycleWordOf(task: Task): ReturnType<typeof lifecycleWord> {
  const state = workerState(task);
  return state === "is working" ? "started" : state === "is waiting" ? "waiting" : state === "stopped" ? "stopped" : state === "failed" ? "failed" : "finished";
}

/** The lead's actions on its team that show as rows. */
export type TeamStepKind = Extract<OrchestratorStepKind, { type: "messaged" | "answered" | "stopped" | "landed" | "reviewed" }>;

export function isTeamStep(kind: { type: string }): kind is TeamStepKind {
  return ["messaged", "answered", "stopped", "landed", "reviewed"].includes(kind.type);
}

/** "’s change", following a name closely. */
const Possessive: FC<{ many: boolean }> = ({ many }) => <span className="-ms-1 shrink-0">{many ? "’ changes" : "’s change"}</span>;

const Quiet: FC<{ children: ReactNode }> = ({ children }) => (
  <p className="bg-code-surface rounded-control text-foreground/80 max-h-60 overflow-y-auto px-3 py-2 whitespace-pre-wrap">{children}</p>
);

/** A row of the lead managing its team: "Messaged Thread rows", "Stopped Fix uploads"; it opens to what it said. */
export const TeamStep: FC<{ kind: TeamStepKind }> = ({ kind }) => {
  let icon: ReactNode;
  let line: ReactNode;
  let detail: ReactNode = undefined;
  switch (kind.type) {
    case "messaged":
      icon = <Chat aria-hidden className="size-4 shrink-0" />;
      line = <>Messaged <WorkerName taskId={kind.taskId} glyph={false} /></>;
      detail = kind.text ? <Quiet>{kind.text}</Quiet> : undefined;
      break;
    case "answered":
      icon = <Chat aria-hidden className="size-4 shrink-0" />;
      line = (
        <>
          Answered <WorkerName taskId={kind.taskId} glyph={false} />
          <span className="-ms-1 shrink-0">’s question</span>
        </>
      );
      detail = (
        <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1">
          {kind.question && (
            <>
              <dt>Asked</dt>
              <dd className="text-foreground/80 whitespace-pre-wrap">{kind.question}</dd>
            </>
          )}
          <dt>Answer</dt>
          <dd className="text-foreground/80 whitespace-pre-wrap">{kind.answer}</dd>
          {kind.why && (
            <>
              <dt>Why</dt>
              <dd className="whitespace-pre-wrap">{kind.why}</dd>
            </>
          )}
        </dl>
      );
      break;
    case "stopped":
      icon = <StopCircle aria-hidden className="size-4 shrink-0" />;
      line = <>Stopped <WorkerName taskId={kind.taskId} glyph={false} /></>;
      detail = kind.reason ? <Quiet>{kind.reason}</Quiet> : undefined;
      break;
    case "landed":
      icon = <Commit aria-hidden className="size-4 shrink-0" />;
      line =
        kind.taskIds.length > 0 ? (
          <>
            Landed <Names taskIds={kind.taskIds} />
            <Possessive many={kind.taskIds.length > 1} />
          </>
        ) : (
          <>Landed the changes</>
        );
      detail = (
        <span>
          {kind.commits} {kind.commits === 1 ? "commit" : "commits"} on {kind.branch}
        </span>
      );
      break;
    case "reviewed":
      icon = <CheckCircle aria-hidden className="size-4 shrink-0" />;
      line = (
        <>
          Reviewed <Names taskIds={kind.taskIds} />
          <Possessive many={kind.taskIds.length > 1} />
        </>
      );
      detail = <span>{kind.findings === 0 ? "No findings" : kind.findings === 1 ? "1 finding" : `${kind.findings} findings`}</span>;
      break;
  }
  return (
    <ThreadActivity data-slot="orchestrator-step" data-kind={kind.type} className={ROW} detail={detail} detailClassName={cn(ROW_DETAIL, "wrap-break-word")}>
      {icon}
      <span className="flex min-w-0 items-center gap-1 truncate">{line}</span>
    </ThreadActivity>
  );
};
