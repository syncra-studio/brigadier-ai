import type { FC } from "react";

import { type Names, setupRows, setupStarting, setupStepWords } from "@/app/conversation/activity/setupWords";
import type { SetupSteps } from "@/app/conversation/blocks";
import { useViewConversation } from "@/app/conversation/viewContext";
import { ROW, ThreadActivity } from "@/components/conversation";
import { Branch, Check, Spinner, X } from "@/components/icons";
import { CLIS } from "@/ipc/generated/clis";
import { cn } from "@/lib/utils";
import { useBoard } from "@/state/board";

const ICONS = { done: Check, running: Spinner, failed: X } as const;

/**
 * A new session's setup as an action line (D208): open with its steps while the turn starts,
 * then one line ("Created a worktree", "Worktree setup failed") that opens to them.
 */
export const SetupRow: FC<{ kind: SetupSteps }> = ({ kind }) => {
  const requestId = kind.steps[0]?.requestId ?? null;
  const starting = useBoard((s) => !!s.board && setupStarting(s.board) && s.board.runRequest === requestId);
  const conversation = useViewConversation();
  const setup = conversation?.setup;
  const environment = setup?.type === "session" ? setup.environment : null;
  const names: Names = {
    base: environment?.type === "newWorktree" ? environment.base : "branch",
    branch: environment?.branch ?? "the branch",
    cli: (setup?.type === "session" && CLIS.find((cli) => cli.id === setup.orchestrator.provider)?.label) || "the CLI",
  };
  const rows = setupRows(kind.steps, starting);
  const last = rows.at(-1);
  const failed = last?.status === "failed";
  // A CLI that didn't start leaves the worktree made: its own row says what failed.
  const title = starting && !failed ? "Creating a worktree" : failed && last?.step !== "cli" ? "Worktree setup failed" : "Created a worktree";
  const list = (
    <div data-slot="setup-steps" className="flex flex-col gap-1">
      {rows.map((row) => {
        const Icon = ICONS[row.status];
        return (
          <div key={row.step} data-step={row.step} data-state={row.status} className="flex min-w-0 flex-col">
            <span className={cn(ROW, row.status === "failed" && "text-destructive")}>
              <Icon aria-hidden className={cn("size-4 shrink-0", row.status === "running" && "animate-spin")} />
              <span className="min-w-0 truncate">{setupStepWords(row.step, row.status, names)}</span>
            </span>
            {row.error !== null && <span className="ps-5.5 break-words whitespace-pre-wrap">{row.error}</span>}
          </div>
        );
      })}
    </div>
  );
  return (
    // Remounted when the start ends, so the open list folds into its one line.
    <ThreadActivity key={starting ? "starting" : "done"} autoOpen={starting} data-slot="setup-line" data-state={starting ? "running" : failed ? "failed" : "done"} detail={list}>
      <Branch aria-hidden className="size-4 shrink-0" />
      <span className={cn("min-w-0 truncate", starting && !failed && "shimmer")}>{title}</span>
    </ThreadActivity>
  );
};
