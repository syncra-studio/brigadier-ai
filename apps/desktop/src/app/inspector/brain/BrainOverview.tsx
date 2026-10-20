import { type ReactNode, useEffect, useState } from "react";

import { KINDS, NODE_KINDS } from "@/app/inspector/brain/kinds";
import { Spinner } from "@/components/icons";
import { Badge } from "@/components/controls/badge";
import { Button } from "@/components/controls/button";
import { useNow } from "@/hooks/use-now";
import type {
  BrainJob,
  BrainJobKind,
  BrainJobState,
  BrainOverview,
  EmbedderState,
  IndexStatus,
} from "@/ipc/generated";
import { brainUseText, lowBrainUse } from "@/lib/brainUse";
import { formatAgo, formatBytes, formatDateTime, formatDuration, formatMs } from "@/lib/format";
import { inspectedConversation } from "@/state/actions";
import {
  loadBrain,
  projectOf,
  rebuildIndex,
  runBrainJob,
  unwatchSessionTasks,
  useBrain,
  watchSessionTasks,
} from "@/state/brain";
import { useApp } from "@/state/store";

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

const count = new Intl.NumberFormat();

function Section({ title, children, aside }: { title: string; children: ReactNode; aside?: ReactNode }) {
  return (
    <section className="flex flex-col gap-2 border-b px-3 py-3 text-xs last:border-b-0">
      <div className="flex items-center gap-2">
        <h2 className="text-muted-foreground flex-1 font-medium">{title}</h2>
        {aside}
      </div>
      {children}
    </section>
  );
}

function Detail({ label, children }: { label: string; children: ReactNode }) {
  return (
    <>
      <dt className="text-muted-foreground">{label}</dt>
      <dd className="min-w-0 truncate text-end tabular-nums">{children}</dd>
    </>
  );
}

function Progress({ value, label }: { value: number | null; label: string }) {
  const share = value === null ? null : Math.min(100, Math.max(0, value * 100));
  return (
    <div
      role="progressbar"
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={share === null ? undefined : Math.round(share)}
      className="bg-muted h-1 overflow-hidden rounded-full"
    >
      {share !== null && <div className="bg-primary h-full" style={{ width: `${share}%` }} />}
    </div>
  );
}

/** A project's Brain (or the Personal Brain) at a glance, kept live by the tab. */
export function BrainOverviewView({
  brainKey,
  overview,
  moving,
}: {
  brainKey: string;
  overview: BrainOverview;
  moving: boolean;
}) {
  const now = useNow(moving ? 1000 : 30_000);
  const { stats } = overview;
  const projectId = projectOf(brainKey);
  return (
    <>
      <Section title="Knowledge">
        <dl className="grid grid-cols-2 gap-x-4 gap-y-1">
          <Detail label="Nodes">{count.format(stats.nodes)}</Detail>
          <Detail label="Edges">{count.format(stats.edges)}</Detail>
          <Detail label="Stale">
            <span className={stats.stale > 0 ? "text-warning" : undefined}>{count.format(stats.stale)}</span>
          </Detail>
          <Detail label="Without an embedding">{count.format(stats.unembedded)}</Detail>
          <Detail label="Transcript entries">{count.format(stats.transcriptEntries)}</Detail>
          <Detail label="Size on disk">{formatBytes(stats.bytesOnDisk)}</Detail>
        </dl>
        <ul aria-label="Nodes by kind" className="grid grid-cols-2 gap-x-4 gap-y-1">
          {NODE_KINDS.map((kind) => {
            const nodes = stats.kinds.find((entry) => entry.kind === kind)?.nodes ?? 0;
            return (
              <li key={kind} className="flex items-center gap-1.5">
                <span aria-hidden className={`${KINDS[kind].swatch} size-2 shrink-0 rounded-full`} />
                <span className={nodes === 0 ? "text-muted-foreground flex-1" : "flex-1"}>
                  {KINDS[kind].label}
                </span>
                <span className="text-muted-foreground tabular-nums">{count.format(nodes)}</span>
              </li>
            );
          })}
        </ul>
      </Section>
      {projectId !== null && (
        <IndexSection index={overview.index} now={now} />
      )}
      {projectId !== null && <WorkersSection projectId={projectId} />}
      <EmbedderSection overview={overview} />
      {projectId !== null && <JobsSection jobs={overview.jobs} now={now} />}
      {projectId !== null && <DeveloperActions brainKey={brainKey} projectId={projectId} hasIndex={overview.index !== null} />}
    </>
  );
}

function indexState(index: IndexStatus): ReactNode {
  switch (index.state.type) {
    case "new":
      return "not scanned yet";
    case "scanning":
      return (
        <span className="inline-flex items-center gap-1.5">
          <Spinner className="size-icon-xs animate-spin" />
          scanning {count.format(index.state.done)} of {count.format(index.state.total)}
        </span>
      );
    case "ready":
      return <span className="text-success">ready</span>;
    case "failed":
      return <span className="text-destructive">failed</span>;
  }
}

function IndexSection({ index, now }: { index: IndexStatus | null; now: number }) {
  if (!index) {
    return (
      <Section title="Code index">
        <p className="text-muted-foreground">This project has no repository to index.</p>
      </Section>
    );
  }
  const languages = index.languages.toSorted((a, b) => b.files - a.files);
  return (
    <Section title="Code index">
      <dl className="grid grid-cols-2 gap-x-4 gap-y-1">
        <Detail label="State">{indexState(index)}</Detail>
        <Detail label="Files">{count.format(index.files)}</Detail>
        <Detail label="Symbols">{count.format(index.symbols)}</Detail>
        <Detail label="References">{count.format(index.references)}</Detail>
        <Detail label="Watching">{index.watching ? "yes" : "no"}</Detail>
        <Detail label="Last scan">
          {index.lastScanAtMs === null ? (
            "never"
          ) : (
            <span title={formatDateTime(index.lastScanAtMs)}>
              {formatAgo(index.lastScanAtMs, now)}
              {index.lastScanMs !== null && ` · ${formatMs(index.lastScanMs)}`}
            </span>
          )}
        </Detail>
        {index.lastScanParsed !== null && (
          <Detail label="Parsed in last scan">{count.format(index.lastScanParsed)} files</Detail>
        )}
        <Detail label="Last change applied">
          {index.updatedAtMs === null ? "—" : formatAgo(index.updatedAtMs, now)}
        </Detail>
      </dl>
      {index.state.type === "scanning" && (
        <Progress
          label="Index scan"
          value={index.state.total > 0 ? index.state.done / index.state.total : null}
        />
      )}
      {index.state.type === "failed" && (
        <p role="alert" className="text-destructive whitespace-pre-wrap">
          {index.state.error}
        </p>
      )}
      {languages.length > 0 && (
        <p className="text-muted-foreground">
          {languages.map((entry) => `${entry.language} ${count.format(entry.files)}`).join(" · ")}
        </p>
      )}
      <p className="text-muted-foreground truncate font-mono" title={index.root}>
        {index.root}
      </p>
    </Section>
  );
}

function embedderState(state: EmbedderState): ReactNode {
  switch (state.type) {
    case "notInstalled":
      return "not installed";
    case "downloading":
      return (
        <span className="inline-flex items-center gap-1.5">
          <Spinner className="size-icon-xs animate-spin" />
          downloading {formatBytes(state.received)}
          {state.total > 0 && ` of ${formatBytes(state.total)}`}
        </span>
      );
    case "installed":
      return "installed, not loaded";
    case "loading":
      return "loading";
    case "loaded":
      return <span className="text-success">loaded</span>;
    case "failed":
      return <span className="text-destructive">failed</span>;
  }
}

function EmbedderSection({ overview }: { overview: BrainOverview }) {
  const { embedder } = overview;
  const state = embedder.state;
  return (
    <Section title="Embedding model">
      <dl className="grid grid-cols-2 gap-x-4 gap-y-1">
        <Detail label="Model">
          <span className="font-mono" title={embedder.model}>
            {embedder.model}
          </span>
        </Detail>
        <Detail label="State">{embedderState(state)}</Detail>
        <Detail label="Size">{formatBytes(embedder.modelBytes)}</Detail>
        <Detail label="Dimensions">{embedder.dimensions}</Detail>
      </dl>
      {state.type === "downloading" && (
        <Progress label="Model download" value={state.total > 0 ? state.received / state.total : null} />
      )}
      {state.type === "failed" && (
        <p role="alert" className="text-destructive whitespace-pre-wrap">
          {state.error}
        </p>
      )}
    </Section>
  );
}

const JOB_KINDS: Record<BrainJobKind, string> = {
  skeleton: "Skeleton pass",
  enrichment: "Enrichment",
};

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

/**
 * How the inspected session's workers used this Brain: their queries, the facts their briefs
 * carried, what they wrote back and their calls. A long task that neither asked nor taught it
 * is marked "Low Brain use": a gap to fix (SESSION-FIXES-PLAN.md Q6.6). The session is the one
 * Settings was opened from; its tasks are read once and kept current by task events.
 */
function WorkersSection({ projectId }: { projectId: string }) {
  const sessionId = useApp((s) => {
    const open = inspectedConversation(s);
    return open?.projectId === projectId ? open.id : null;
  });
  const tasks = useBrain((s) =>
    sessionId !== null && s.sessionTasks?.conversationId === sessionId ? s.sessionTasks.tasks : null,
  );
  // Read on showing; the bridge's resync reads again on reconnecting or after missed events.
  useEffect(() => {
    if (sessionId === null) return undefined;
    watchSessionTasks(sessionId).catch((cause: unknown) => {
      console.error("reading the session's workers failed", cause);
    });
    return () => unwatchSessionTasks(sessionId);
  }, [sessionId]);
  if (!tasks) return null;
  const workers = Object.values(tasks)
    .filter((task) => task.brain)
    .toSorted((a, b) => a.number - b.number);
  if (workers.length === 0) return null;
  return (
    <Section title="Workers in this session">
      <ul className="divide-border flex flex-col divide-y">
        {workers.map((task) => (
          <li key={task.id} data-task={`task-${task.number}`} className="flex flex-col gap-0.5 py-1.5">
            <div className="flex items-center gap-2">
              <span className="min-w-0 flex-1 truncate font-medium">
                task-{task.number} {task.title}
              </span>
              {lowBrainUse(task.brain) && <Badge variant="warning">Low Brain use</Badge>}
            </div>
            <p className="text-muted-foreground tabular-nums">{brainUseText(task.brain)}</p>
          </li>
        ))}
      </ul>
    </Section>
  );
}

function JobRow({ job, now }: { job: BrainJob; now: number }) {
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

function JobsSection({ jobs, now }: { jobs: readonly BrainJob[]; now: number }) {
  return (
    <Section title="Brain jobs">
      {jobs.length === 0 ? (
        <p className="text-muted-foreground">No skeleton pass or enrichment has run yet.</p>
      ) : (
        <ul className="divide-border flex flex-col divide-y">
          {jobs.map((job) => (
            <JobRow key={job.id} job={job} now={now} />
          ))}
        </ul>
      )}
    </Section>
  );
}

type Action = "skeleton" | "enrichment" | "rebuild";

function DeveloperActions({
  brainKey,
  projectId,
  hasIndex,
}: {
  brainKey: string;
  projectId: string;
  hasIndex: boolean;
}) {
  const [running, setRunning] = useState<Action | null>(null);
  const [result, setResult] = useState<{ tone: "done" | "error"; text: string } | null>(null);
  const run = (action: Action) => {
    setRunning(action);
    setResult(null);
    const started =
      action === "rebuild"
        ? rebuildIndex(projectId).then(() => "Rebuilding the index.")
        : runBrainJob(projectId, action).then((jobId) => `Started ${JOB_KINDS[action].toLowerCase()} ${jobId}.`);
    started
      .then((text) => {
        setResult({ tone: "done", text });
        return loadBrain(brainKey);
      })
      .catch((cause: unknown) => setResult({ tone: "error", text: errorText(cause) }))
      .finally(() => setRunning(null));
  };
  return (
    <Section title="Developer">
      <div className="flex flex-wrap gap-2">
        <Button size="xs" variant="outline" disabled={running !== null} onClick={() => run("skeleton")}>
          {running === "skeleton" && <Spinner className="animate-spin" />}
          Run skeleton pass
        </Button>
        <Button size="xs" variant="outline" disabled={running !== null} onClick={() => run("enrichment")}>
          {running === "enrichment" && <Spinner className="animate-spin" />}
          Enrich now
        </Button>
        <Button
          size="xs"
          variant="outline"
          disabled={running !== null || !hasIndex}
          onClick={() => run("rebuild")}
        >
          {running === "rebuild" && <Spinner className="animate-spin" />}
          Rebuild index
        </Button>
      </div>
      {result && (
        <p role={result.tone === "error" ? "alert" : "status"} className={result.tone === "error" ? "text-destructive" : "text-muted-foreground"}>
          {result.text}
        </p>
      )}
    </Section>
  );
}
