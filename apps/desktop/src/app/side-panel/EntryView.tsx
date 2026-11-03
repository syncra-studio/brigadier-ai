import { receivedMessage } from "@/app/conversation/workerPresentation";
import { CompactionDivider } from "@/app/conversation/CompactionRow";
import { ActionRow } from "@/app/side-panel/ActionRow";
import { REFRESHED } from "@/app/side-panel/workerSeams";
import { WorkerMarkdown } from "@/app/side-panel/WorkerMarkdown";
import { ROW, ErrorState, shownCommand, type ThreadEntry } from "@/components/conversation";
import { ShieldCheck } from "@/components/icons";
import type { ErrorKind } from "@/ipc/generated";
import { cn } from "@/lib/utils";

/** A worker's error in a few words, by kind; its own words follow in full. */
const ERROR_TITLES: Record<ErrorKind, string> = {
  usageLimit: "Out of usage for now",
  rateLimit: "Rate limited",
  overloaded: "The model is overloaded",
  auth: "Not signed in",
  billing: "A billing problem",
  contextWindow: "Out of context",
  invalidRequest: "The request was refused",
  policy: "Blocked by a usage policy",
  network: "A network problem",
  sandbox: "Blocked by the sandbox",
  server: "The provider had an error",
  process: "The CLI stopped",
  other: "Something went wrong",
  stalled: "It stopped responding",
};

const row = ROW;

/** One entry of a worker's thread: its actions, a message, a pending approval, an error or a notice. */
export function EntryView({ entry, sent }: { entry: ThreadEntry; sent: readonly string[] }) {
  if (entry.kind === "actions") return <>{entry.items.map((item) => <ActionRow key={item.key} item={item} />)}</>;
  const { item } = entry;
  switch (item.kind) {
    case "reasoning":
      // A settled thought sits in its work group; the live one is the live line.
      return null;
    case "message": {
      // A reply still streaming shows from its first word.
      if (item.streaming && !item.text.trim()) return null;
      return item.role === "user" ? (
        <div data-slot="worker-received" className="bg-secondary rounded-thread min-w-0 max-w-full self-end px-3 py-2 text-sm whitespace-pre-wrap">
          {receivedMessage(item.text, sent) ?? item.text}
        </div>
      ) : (
        <div className="text-foreground min-w-0 leading-relaxed wrap-anywhere">
          <WorkerMarkdown text={item.text} streaming={item.streaming} />
        </div>
      );
    }
    case "approval": {
      // Its orchestrator answers it (D48); answered by anyone else, the command's own row tells what came of it.
      const { resolution } = item;
      if (resolution && resolution.decidedBy !== "orchestrator") return null;
      const what = item.request.command ? shownCommand(item.request.command) : item.request.paths.join(", ") || item.request.tool;
      const reason = resolution?.decision.type === "deny" ? resolution.decision.message.trim() : "";
      const text = !resolution
        ? `Waiting for the orchestrator: ${what}`
        : resolution.decision.type === "deny"
          ? `Denied by orchestrator${reason ? `: ${reason}` : ""}`
          : `Allowed by orchestrator: ${what}`;
      return (
        <div data-slot="worker-approval" className={cn(row, resolution ? "text-muted-foreground" : "text-warning")} title={resolution ? `${text}\n${what}` : text}>
          <ShieldCheck aria-hidden className="size-4 shrink-0" />
          <span className="min-w-0 truncate">{text}</span>
        </div>
      );
    }
    case "error":
      // A usage limit is no error of its turn: it switches on, or its state says the wait (D180).
      if (item.error.kind === "usageLimit" && !item.error.willRetry) return null;
      return (
        <ErrorState
          summary={item.error.willRetry ? `${ERROR_TITLES[item.error.kind]}: it tries again on its own.` : `${ERROR_TITLES[item.error.kind]}.`}
          details={item.error.message}
        />
      );
    case "notice":
      // Its compactions, as the thread's divider ([`withSeams`]).
      if (item.text === REFRESHED) return <CompactionDivider compaction={{ id: item.key, inTurn: false, automatic: true, state: "done", error: null, position: 0, startedAtMs: 0, endedAtMs: 0 }} />;
      return (
        <p className={cn("text-sm", item.level === "warning" ? "text-warning" : "text-muted-foreground")}>
          {item.text}
        </p>
      );
    case "turnCompleted":
      return null;
    default:
      return null;
  }
}
