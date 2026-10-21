import { count, size } from "@/app/settings/storage/model";
import { History, Sparkles, Spinner } from "@/components/icons";
import { Button, RoundBadge } from "@/components/controls";
import type { ProjectUsage } from "@/ipc/generated";
import { cn } from "@/lib/utils";

/** How freeing up the projects not in use goes. */
export type UnusedFreeUp =
  | { phase: "idle" }
  | { phase: "confirm" }
  | { phase: "freeing" }
  | { phase: "done"; title: string; line: string }
  | { phase: "failed"; line: string };

/** The projects not used for three months: what their Brains and conversations take, and
 * freeing that up (after a confirmation) with the projects, their repositories and files kept. */
export function UnusedRow({
  projects,
  bytes,
  state,
  onState,
  onFreeUp,
}: {
  projects: ProjectUsage[];
  bytes: number;
  state: UnusedFreeUp;
  onState: (state: UnusedFreeUp) => void;
  onFreeUp: () => void;
}) {
  const n = projects.length;
  const title =
    state.phase === "done"
      ? state.title
      : n === 0
        ? "Projects not used in 3 months"
        : `${count(n, "project", "projects")} not used in 3 months`;
  const line =
    state.phase === "confirm"
      ? `Delete ${n === 1 ? "its" : "their"} ${count(
          projects.reduce((total, project) => total + project.conversations, 0),
          "conversation",
          "conversations",
        )} and move ${n === 1 ? "its Brain" : "their Brains"} to the Trash? The ${n === 1 ? "project" : "projects"}, your repositories and files stay.`
      : state.phase === "done" || state.phase === "failed"
        ? state.line
        : `${n === 1 ? "Its" : "Their"} brains and conversations take ${size(bytes)}. Your repositories and files stay.`;
  return (
    <div data-unused className="border-divider flex items-center gap-4 border-b py-3.5">
      <RoundBadge>
        <History />
      </RoundBadge>
      <div className="flex min-w-0 flex-1 flex-col gap-1">
        <span className="text-foreground text-label font-medium">{title}</span>
        <span className={cn("text-xs", state.phase === "failed" ? "text-destructive" : "text-foreground/65")}>
          {line}
        </span>
      </div>
      {state.phase === "confirm" ? (
        <>
          <Button variant="ghost" size="sm" onClick={() => onState({ phase: "idle" })}>Cancel</Button>
          <Button size="sm" onClick={onFreeUp}>Free up {size(bytes)}</Button>
        </>
      ) : state.phase === "done" || n === 0 ? null : state.phase === "freeing" ? (
        <Button variant="soft" size="sm" disabled>
          <Spinner className="animate-spin motion-reduce:animate-none" />
          Freeing up…
        </Button>
      ) : (
        <Button variant="soft" size="sm" onClick={() => onState({ phase: "confirm" })}>
          <Sparkles aria-hidden />
          Free up {size(bytes)}
        </Button>
      )}
    </div>
  );
}
