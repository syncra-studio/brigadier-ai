import { memo, type ReactNode } from "react";

import type { TranscriptItem } from "@/components/transcript/transcript";
import { Badge } from "@/components/ui/badge";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import { ChevronRight } from "@/components/ui/icon";
import type { Decider, ProviderError } from "@/ipc/generated";
import { formatDateTime, formatMs } from "@/lib/format";
import { cn } from "@/lib/utils";

/** Who answered an approval, as shown to the user. */
export const DECIDERS: Record<Decider, string> = {
  policy: "Brigadier",
  user: "you",
  recorded: "the recording",
};

/** One transcript row. */
export const TranscriptRow = memo(function TranscriptRow({ item }: { item: TranscriptItem }) {
  switch (item.kind) {
    case "message":
      return (
        <div
          className={cn(
            "rounded-lg px-2.5 py-1.5 whitespace-pre-wrap",
            item.role === "user" ? "bg-secondary self-end" : "bg-card border",
          )}
        >
          {item.text}
          {item.streaming && <span className="text-muted-foreground"> ▍</span>}
        </div>
      );
    case "reasoning":
      return (
        <Details summary={item.streaming ? "Thinking…" : "Reasoning"} muted>
          <p className="text-muted-foreground whitespace-pre-wrap">{item.text}</p>
        </Details>
      );
    case "command":
      return (
        <Details
          summary={
            <>
              <span className="font-mono">$ {item.command || "command"}</span>
              <StatusBadge status={item.status} />
              {item.exitCode !== null && (
                <span className="text-muted-foreground">exit {item.exitCode}</span>
              )}
              {item.durationMs !== null && (
                <span className="text-muted-foreground">{formatMs(item.durationMs)}</span>
              )}
            </>
          }
        >
          {item.cwd && <p className="text-muted-foreground font-mono">{item.cwd}</p>}
          {item.output && (
            <pre className="bg-code-surface rounded-control max-h-60 overflow-auto p-2 font-mono whitespace-pre-wrap">
              {item.output}
            </pre>
          )}
        </Details>
      );
    case "tool":
      return (
        <Details
          summary={
            <>
              <span className="font-mono">{item.name}</span>
              <StatusBadge status={item.status} />
            </>
          }
        >
          {item.input && (
            <pre className="bg-code-surface rounded-control max-h-40 overflow-auto p-2 font-mono whitespace-pre-wrap">
              {item.input}
            </pre>
          )}
          {item.output && (
            <pre className="bg-code-surface rounded-control max-h-40 overflow-auto p-2 font-mono whitespace-pre-wrap">
              {item.output}
            </pre>
          )}
        </Details>
      );
    case "files":
      return (
        <div className="flex flex-col gap-0.5">
          <div className="flex items-center gap-2">
            <span>File changes</span>
            <StatusBadge status={item.status} />
          </div>
          {item.changes.map((change) => (
            <span key={change.path} className="text-muted-foreground font-mono">
              {change.kind} {change.path}
            </span>
          ))}
        </div>
      );
    case "image":
      return (
        <div className="flex items-center gap-2">
          <span>Image</span>
          <StatusBadge status={item.status} />
          <span className="text-muted-foreground min-w-0 truncate font-mono">
            {item.path ?? item.prompt}
          </span>
        </div>
      );
    case "approval":
      return (
        <div className="flex flex-wrap items-center gap-2">
          <Badge variant={item.request.escalation ? "warning" : "outline"}>
            approval · {item.request.kind}
          </Badge>
          <span className="min-w-0 flex-1 truncate font-mono">
            {item.request.command ?? (item.request.paths.join(", ") || item.request.tool)}
          </span>
          {item.resolution ? (
            <span className="text-muted-foreground">
              {item.resolution.decision.type === "deny" ? "denied" : "allowed"} by{" "}
              {DECIDERS[item.resolution.decidedBy]}
            </span>
          ) : (
            <span className="text-warning">waiting</span>
          )}
        </div>
      );
    case "turnStarted":
      return <Divider label="Turn started" />;
    case "turnCompleted":
      return (
        <Divider
          label={`Turn ${item.status}${item.durationMs === null ? "" : ` · ${formatMs(item.durationMs)}`}`}
          tone={item.status === "completed" ? "muted" : "warning"}
        />
      );
    case "error":
      return <ErrorCard error={item.error} />;
    case "notice":
      return (
        <p className={item.level === "warning" ? "text-warning" : "text-muted-foreground"}>
          {item.text}
        </p>
      );
  }
});

export function Details({
  summary,
  muted,
  children,
}: {
  summary: ReactNode;
  muted?: boolean;
  children: ReactNode;
}) {
  return (
    <Collapsible>
      <CollapsibleTrigger
        className={cn(
          "hover:text-foreground group flex max-w-full items-center gap-1.5 text-start",
          muted && "text-muted-foreground",
        )}
      >
        <ChevronRight className="size-icon-xs shrink-0 transition-transform group-data-[state=open]:rotate-90" />
        {summary}
      </CollapsibleTrigger>
      <CollapsibleContent className="mt-1 flex flex-col gap-1 ps-4">{children}</CollapsibleContent>
    </Collapsible>
  );
}

export function StatusBadge({ status }: { status: string }) {
  const variant =
    status === "completed"
      ? "success"
      : status === "inProgress"
        ? "warning"
        : status === "failed"
          ? "destructive"
          : "secondary";
  return <Badge variant={variant}>{status}</Badge>;
}

export function Divider({ label, tone = "muted" }: { label: string; tone?: "muted" | "warning" }) {
  return (
    <div
      className={cn(
        "flex items-center gap-2",
        tone === "warning" ? "text-warning" : "text-muted-foreground",
      )}
    >
      <span className="bg-border h-px flex-1" />
      <span>{label}</span>
      <span className="bg-border h-px flex-1" />
    </div>
  );
}

/** A classified provider error, with the limit it hit when it is one. */
export function ErrorCard({ error }: { error: ProviderError }) {
  return (
    <div className="border-destructive/50 bg-destructive/10 flex flex-col gap-1 rounded-lg border p-2.5">
      <div className="flex flex-wrap items-center gap-2">
        <Badge variant="destructive">{error.kind}</Badge>
        {error.code && <span className="text-muted-foreground font-mono">{error.code}</span>}
        {error.willRetry && <Badge variant="secondary">CLI retries</Badge>}
      </div>
      {error.limit && (
        <p>
          Window <span className="font-mono">{error.limit.window ?? "unknown"}</span>
          {error.limit.resetsAtMs !== null &&
            ` · resets ${formatDateTime(error.limit.resetsAtMs)}`}
        </p>
      )}
      <p className="whitespace-pre-wrap">{error.message}</p>
    </div>
  );
}
