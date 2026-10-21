import { count, JOB_KINDS } from "@/app/inspector/brain/overviewText";
import { Spinner } from "@/components/icons";
import { Badge } from "@/components/controls";
import type { BrainJob, BrainJobState } from "@/ipc/generated";
import { formatAgo, formatDateTime, formatDuration } from "@/lib/format";

function jobBadge(state: BrainJobState): { label: string; variant: "success" | "secondary" | "warning" | "destructive" } {
  switch (state.type) {
    case "running":
      return { label: "running", variant: "warning" };
    case "done":
      return { label: "done", variant: "success" };
    case "stopped":
      return { label: "stopped", variant: "secondary" };
    case "failed":
      return { label: "failed", variant: "destructive" };
  }
}

export function JobRow({ job, now }: { job: BrainJob; now: number }) {
  const badge = jobBadge(job.state);
  const ended = job.endedAtMs ?? now;
  const reason =
    job.state.type === "failed" ? job.state.error : job.state.type === "stopped" ? job.state.reason : null;
  return (
    <li className="flex flex-col gap-0.5 py-1.5">
      <div className="flex items-center gap-2">
        <span className="font-medium">{JOB_KINDS[job.kind]}</span>
        <Badge variant={badge.variant}>
          {job.state.type === "running" && <Spinner className="animate-spin" />}
          {badge.label}
        </Badge>
        <span className="text-muted-foreground min-w-0 flex-1 truncate text-end font-mono">
          {job.provider}
          {job.model && ` · ${job.model}`}
        </span>
      </div>
      <div className="text-muted-foreground flex items-center gap-2 tabular-nums">
        <span title={formatDateTime(job.startedAtMs)}>{formatAgo(job.startedAtMs, now)}</span>
        <span>· {formatDuration(Math.max(0, ended - job.startedAtMs))}</span>
        <span>· {count.format(job.nodes)} nodes</span>
      </div>
      {job.note && <p className="text-muted-foreground">{job.note}</p>}
      {reason && (
        <p className={job.state.type === "failed" ? "text-destructive" : "text-muted-foreground"}>{reason}</p>
      )}
    </li>
  );
}
