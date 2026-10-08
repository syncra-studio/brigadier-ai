import { useVirtualizer } from "@tanstack/react-virtual";
import { memo, useLayoutEffect, useRef, useState } from "react";

import { Button } from "@/components/ui/button";
import type { DomainEvent, EventEnvelope } from "@/ipc/generated";
import { formatClock, formatMs } from "@/lib/format";
import { imagePreview } from "@/lib/inlineImages";
import { probeSamples, summarize } from "@/lib/perf";
import { tokenPx } from "@/lib/tokens";
import { runProbeBurst } from "@/state/actions";
import { INSPECTOR_EVENTS, useApp } from "@/state/store";

function summary(event: DomainEvent): string {
  switch (event.type) {
    case "projectCreated":
      return event.project.name;
    case "conversationCreated":
      return `${event.conversation.kind} “${event.conversation.title}”`;
    case "conversationRenamed":
      return `“${event.title}”`;
    case "conversationPinned":
      return event.pinnedAtMs === null ? "unpinned" : "pinned";
    case "conversationFallback":
      return event.fallback === null
        ? "chosen model back"
        : `stand-in ${event.fallback.choice.provider} ${event.fallback.choice.model ?? "default"}`;
    case "conversationWaiting":
      return event.wait === null ? "no longer waiting" : `waits: ${event.wait.reason}`;
    case "messageAppended":
      return imagePreview(event.message.text, event.message.attachments).slice(0, 120).replace(/\s+/g, " ");
    case "settingsChanged":
      return `density ${event.settings.density}`;
    case "probe":
      return `probe ${event.index + 1}/${event.count}`;
    case "draftPinned":
      return `${event.scope}: ${event.attachments.length} attachments`;
    case "rawSessionCreated":
      return `${event.session.provider} ${event.session.source.type}`;
    case "rawSessionUpdated":
      return event.state;
    case "rawEvent":
      return event.event.type;
    case "cleanupRecorded":
      return event.artifact.type;
    case "cleanupRemoved":
      return `${event.artifacts.length} removed`;
    case "branchesKept":
      return `${event.branches.length} kept`;
    case "cleanupRequested":
      return event.owner;
    case "cleanupCompleted":
      return event.failures.length === 0 ? "removed" : `${event.failures.length} failed`;
    case "rankingsChanged":
      return "model rankings changed";
    case "providerChecked":
      return event.overview.provider;
    case "accountsChecked":
      return `${event.accounts.accounts.length} accounts`;
    case "updatesChanged":
      return event.updates.items.length === 0
        ? "up to date"
        : event.updates.items.map((item) => `${item.target} ${item.latest}`).join(", ");
    case "projectUpdated":
      return event.project.name;
    case "conversationSetUp":
      return event.setup.type;
    case "conversationLifecycleChanged":
      return event.lifecycle;
    case "conversationCleanup":
      return event.pending ? "cleaning up" : "cleaned up";
    case "conversationDeleting":
      return "deleting";
    case "conversationDeleted":
    case "projectRemoved":
      return "deleted";
    case "engineSwitching":
      return `${event.engine}: deleting ${event.conversations.length}`;
    case "engineSwitched":
      return event.engine;
    case "thinkingDelta":
    case "messageDelta":
      return event.text.slice(0, 120).replace(/\s+/g, " ");
    case "runStateChanged":
      return event.error === null ? event.state : `${event.state}: ${event.error}`;
    case "requestUpdated":
      return `request ${event.request.state.type}`;
    case "workerStepped":
      return `${event.step.kind} ${event.step.taskId}`;
    case "orchestratorStepped":
      return event.step.kind.type;
    case "machineStepped":
      return event.step.command ? `${event.step.kind} ${event.step.command}` : event.step.kind;
    case "compactionUpdated":
      return `compaction ${event.compaction.state.type}`;
    case "messageRated":
      return `${event.rating} ${event.subject}`;
    case "branchSwitched":
      return `branch → ${event.head}`;
    case "conversationNotice":
      return event.notice.text;
    case "taskUpdated":
      return `task-${event.task.number} ${event.task.state}`;
    case "approvalUpdated":
      return `${event.approval.subject.type} ${event.approval.state.type}`;
    case "questionUpdated":
      return event.question.answer === null ? "asked" : "answered";
    case "planUpdated":
      return `“${event.plan.title}” ${event.plan.state.type}`;
    case "reviewUpdated":
      return `${event.review.kind} review ${event.review.state.type}`;
    case "threadCommitsSeen":
      return `${event.branch} seen at ${event.tip.slice(0, 10)}`;
    case "outputStored":
      return `${event.output.alias} ${event.output.source} ${event.output.bytes} B`;
    case "checkRan":
      return `${event.command} ${event.cached ? "cached" : event.status}${event.bypassed ? ` (not cached: ${event.bypassed})` : ""}`;
    case "threadLooked":
      return `${event.reads.length} read${event.reads.length === 1 ? "" : "s"}, ${event.searches.length} search${event.searches.length === 1 ? "" : "es"}`;
    case "previewUpdated":
      return `${event.preview.id} ${event.preview.state.type}`;
    case "overnightUpdated":
      return `“${event.run.name}” ${event.run.state} r${event.run.revision}`;
    case "queueChanged":
      return `${event.queue.items.length} queued${event.queue.paused ? ", paused" : ""}`;
    case "workerEvent":
      return event.event.type;
    case "orchestratorLogged":
      switch (event.entry.type) {
        case "injection":
          return `${event.entry.injection.kind} ${event.entry.injection.label}`;
        case "provider":
          return event.entry.event.type;
        case "rebirth":
          return `rebirth ${event.entry.record.generation}`;
        case "contractBreach":
          return event.entry.message;
      }
      break;
    case "brainJobUpdated":
      return `${event.job.kind} ${event.job.state.type}`;
    case "memoryUpdated":
      return `${event.memory.forgotten ? "forgot" : "saved"} “${event.memory.text.slice(0, 80)}”`;
    case "decidedForYou":
      return event.decision.what;
    case "waitingOnYou":
      return event.item.what;
    case "waitingResolved":
      return `done by ${event.by}`;
  }
}

const EventRow = memo(function EventRow({ envelope }: { envelope: EventEnvelope }) {
  return (
    <div className="h-row-sm flex items-center gap-2 px-3 font-mono text-xs">
      <span className="text-muted-foreground w-12 shrink-0 text-end tabular-nums">
        {envelope.seq}
      </span>
      <span className="text-muted-foreground shrink-0 tabular-nums">
        {formatClock(envelope.atMs)}
      </span>
      <span className="text-foreground shrink-0">{envelope.event.type}</span>
      <span className="text-muted-foreground min-w-0 flex-1 truncate" title={envelope.stream}>
        {envelope.stream} · {summary(envelope.event)}
      </span>
    </div>
  );
});

function useProbeBurst() {
  const [state, setState] = useState<{
    burstId: string;
    count: number;
    painted: number;
    p95: number;
  } | null>(null);
  const [busy, setBusy] = useState(false);

  const run = async () => {
    setBusy(true);
    try {
      const burst = await runProbeBurst();
      const started = performance.now();
      // Poll the painted count until the burst is done (or clearly stalled).
      await new Promise<void>((resolve) => {
        const tick = () => {
          const samples = probeSamples(burst.burstId);
          setState({
            burstId: burst.burstId,
            count: burst.count,
            painted: samples.length,
            p95: summarize(samples).p95Ms,
          });
          if (samples.length >= burst.count || performance.now() - started > 15_000) {
            resolve();
          } else {
            setTimeout(tick, 100);
          }
        };
        tick();
      });
    } finally {
      setBusy(false);
    }
  };
  return { state, busy, run };
}

export function EventsTab() {
  const events = useApp((s) => s.inspector.events);
  const density = useApp((s) => s.settings.density);
  const scrollRef = useRef<HTMLDivElement>(null);
  const probe = useProbeBurst();

  // The app does not use React Compiler, so the virtualizer's unmemoizable API is fine here.
  // oxlint-disable-next-line react/incompatible-library
  const virtualizer = useVirtualizer({
    count: events.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => tokenPx("--spacing-row-sm"),
    getItemKey: (index) => events[index]?.seq ?? index,
    overscan: 10,
  });

  // Row height is a density token.
  useLayoutEffect(() => {
    virtualizer.measure();
  }, [density, virtualizer]);

  return (
    <>
      <div className="flex shrink-0 items-center gap-2 border-b px-3 py-2 text-xs">
        <span className="text-muted-foreground flex-1">
          Live, newest first · last {Math.min(events.length, INSPECTOR_EVENTS)}
        </span>
        {probe.state && (
          <span className="text-muted-foreground tabular-nums">
            {probe.state.painted}/{probe.state.count} painted · p95{" "}
            {formatMs(probe.state.p95)}
          </span>
        )}
        <Button
          size="xs"
          variant="outline"
          disabled={probe.busy}
          onClick={() => void probe.run()}
        >
          Run probe burst
        </Button>
      </div>
      <div ref={scrollRef} data-selectable className="min-h-0 flex-1 overflow-y-auto">
        {events.length === 0 ? (
          <p className="text-muted-foreground p-4 text-xs">
            Events appear here as the core commits them.
          </p>
        ) : (
          <div className="relative w-full" style={{ height: `${virtualizer.getTotalSize()}px` }}>
            {virtualizer.getVirtualItems().map((item) => {
              const envelope = events[item.index];
              if (!envelope) return null;
              return (
                <div
                  key={item.key}
                  className="absolute inset-x-0 top-0"
                  style={{ transform: `translateY(${item.start}px)` }}
                >
                  <EventRow envelope={envelope} />
                </div>
              );
            })}
          </div>
        )}
      </div>
    </>
  );
}
