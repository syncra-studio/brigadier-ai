import { shownText } from "@/app/conversation/streamRelease";
import { receivedMessage } from "@/app/conversation/workerPresentation";
import { ActionRow } from "@/app/side-panel/ActionRow";
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
      // A reply still streaming shows its finished lines only, none until its first (D9 b).
      const shown = shownText(item.text, item.streaming);
      if (item.streaming && !shown) return null;
      return item.role === "user" ? (
        <div data-slot="worker-received" className="bg-secondary rounded-thread min-w-0 max-w-full self-end px-3 py-2 text-sm whitespace-pre-wrap">
          {receivedMessage(item.text, sent) ?? item.text}
        </div>
      ) : (
        <div className="text-foreground min-w-0 leading-relaxed wrap-anywhere">
          <WorkerMarkdown text={shown} streaming={item.streaming} />
        </div>
      );
    }
    case "approval": {
      // Answered, it leaves no trace: the command's own row tells what came of it.
      if (item.resolution) return null;
      const what = item.request.command ? shownCommand(item.request.command) : item.request.paths.join(", ") || item.request.tool;
      return (
        <div className={cn(row, "text-warning")}>
          <ShieldCheck aria-hidden className="size-4 shrink-0" />
          <span className="min-w-0 truncate">Waiting for your approval: {what}</span>
        </div>
      );
    }
    case "error":
      return (
        <ErrorState
          summary={item.error.willRetry ? `${ERROR_TITLES[item.error.kind]}: it tries again on its own.` : `${ERROR_TITLES[item.error.kind]}.`}
          details={item.error.message}
        />
      );
    case "notice":
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
