import { type ReactNode, useEffect, useState } from "react";

import type { LeadStep } from "@/app/conversation/activity/group";
import { type ActionItem, editHunks, fileDiffs, hunkDiff, parsedInput, toolName, type ToolStep, unwrapCommand } from "@/app/conversation/activity/words";
import { ErrorState } from "@/components/assistant-ui/elements/error-state";
import { searchResults, WebSearch } from "@/components/assistant-ui/elements/web-search";
import type { ItemStatus, ThreadItem } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { getThreadItem } from "@/state/actions";

/**
 * What a step of the thread opens to (THREAD-UX-PLAN.md §3.2), the same for the lead's row and a
 * worker's: a Shell box for a command, a small diff for an edit, the results of a search, and the
 * call's input and output for any other tool.
 */

const BOX = "border-border bg-code-surface rounded-control mt-1 flex flex-col border text-sm";
const BODY = "text-code max-h-60 overflow-auto px-3 py-1 font-mono whitespace-pre-wrap";

/** Output this long shows its head and tail, with the lines between folded. */
const LONG_LINES = 40;
const SHOWN_LINES = 15;

/** A long output as its first and last lines, "… N lines …" between them, until opened in full. */
function Output({ text }: { text: string }) {
  const [all, setAll] = useState(false);
  const lines = text.split("\n");
  if (all || lines.length <= LONG_LINES) return <>{text}</>;
  const hidden = lines.length - 2 * SHOWN_LINES;
  return (
    <>
      {lines.slice(0, SHOWN_LINES).join("\n")}
      {"\n"}
      <button type="button" className="text-muted-foreground hover:text-foreground underline-offset-2 hover:underline" onClick={() => setAll(true)}>
        … {hidden} lines …
      </button>
      {"\n"}
      {lines.slice(-SHOWN_LINES).join("\n")}
    </>
  );
}

/** How a command ended, in the Shell box's last line. */
function ending(status: ItemStatus, exit: number | null): string {
  if (status === "inProgress") return "Running";
  if (status === "declined") return "Stopped";
  if (exit !== null && exit !== 0) return `Exit ${exit}`;
  if (status === "failed") return "Failed";
  return "✓ Success";
}

/** A command's box: "Shell", `$ command` and what it printed, then how it ended. */
export function ShellBox({ command, output, status, exit }: {
  command: string;
  output: string | null;
  status: ItemStatus;
  exit: number | null;
}) {
  const printed = output?.trimEnd() ?? "";
  const end = ending(status, exit);
  return (
    <div data-slot="shell-card" className={BOX}>
      <span className="text-muted-foreground px-3 pt-2 text-xs">Shell</span>
      <pre className={BODY}>
        {`$ ${unwrapCommand(command)}`}
        {printed ? (
          <>
            {"\n"}
            <Output text={printed} />
          </>
        ) : (
          <span className="text-muted-foreground">{"\nNo output"}</span>
        )}
      </pre>
      <span
        className={cn(
          "border-border border-t px-3 py-1.5 text-end text-xs",
          end === "✓ Success" ? "text-muted-foreground" : end === "Running" ? "shimmer" : "text-destructive",
        )}
      >
        {end}
      </span>
    </div>
  );
}

/** A titled box of mono text: a tool's input and output, or a list of files. */
export function ToolBox({ title, body }: { title: string; body: string }) {
  return (
    <div data-slot="tool-card" className={BOX}>
      <span className="text-muted-foreground px-3 pt-2 text-xs">{title}</span>
      <pre className={cn(BODY, "pb-2")}>
        <Output text={body} />
      </pre>
    </div>
  );
}

/** One file's change as diff lines: `-` out, `+` in, `@@` between the places it changed. */
function DiffBox({ path, diff }: { path: string; diff: string }) {
  const lines = diff.replace(/\n$/, "").split("\n").filter((line) => !/^(---|\+\+\+) /.test(line));
  const added = lines.filter((line) => line.startsWith("+")).length;
  const removed = lines.filter((line) => line.startsWith("-")).length;
  return (
    <div data-slot="diff-card" className={BOX}>
      <span className="text-muted-foreground flex gap-2 px-3 pt-2 text-xs">
        <span className="min-w-0 truncate">{path.split("/").pop()}</span>
        <span className="text-success tabular-nums">+{added}</span>
        <span className="text-destructive tabular-nums">−{removed}</span>
      </span>
      <pre className={cn(BODY, "pb-2")}>
        {lines.map((line, index) => (
          <span
            key={index}
            className={cn(
              "block",
              line.startsWith("+") && "bg-success/10",
              line.startsWith("-") && "bg-destructive/10",
              line.startsWith("@@") && "text-muted-foreground",
            )}
          >
            {line.startsWith("@@") ? "⋯" : line || " "}
          </span>
        ))}
      </pre>
    </div>
  );
}

/** Several files' changes, each its own box. */
function Diffs({ files }: { files: readonly { path: string; diff: string }[] }) {
  return (
    <div className="flex flex-col gap-1">
      {files.map((file, index) => (
        <DiffBox key={`${file.path}:${index}`} path={file.path} diff={file.diff} />
      ))}
    </div>
  );
}

/** A web search's query, from its call. */
function searchQuery(input: string | null): string | null {
  const query = parsedInput(input)?.["query"];
  return typeof query === "string" ? query : null;
}

const SHELLS = new Set(["Bash", "run", "run_unsandboxed", "shell", "exec_command", "run_check"]);
const EDITS = new Set(["Edit", "MultiEdit", "Write", "NotebookEdit", "apply_patch"]);

/** A call's detail from what it was given and what it gave back. */
function callDetail(name: string, status: ItemStatus, call: { input: string | null; output: string | null; exit: number | null; command?: string | null }): ReactNode {
  const short = toolName(name);
  if (SHELLS.has(short)) {
    const given = parsedInput(call.input)?.["command"];
    const command = call.command ?? (typeof given === "string" ? given : "");
    if (command) return <ShellBox command={command} output={call.output} status={status} exit={call.exit} />;
  }
  if (EDITS.has(short)) {
    const hunks = editHunks(name, call.input);
    if (hunks.length > 0) return <Diffs files={hunks.map((hunk) => ({ path: hunk.path, diff: hunkDiff(hunk) }))} />;
  }
  const query = short === "WebSearch" || short === "web_search" ? searchQuery(call.input) : null;
  if (query !== null && status === "failed") {
    // A failed search found nothing: its query, then what went wrong in full.
    return (
      <div className="flex flex-col gap-2 pt-1">
        <WebSearch query={query} results={[]} />
        <ErrorState title="The search failed" detail={call.output || null} />
      </div>
    );
  }
  if (query !== null) {
    return <WebSearch className="pt-1" query={query} results={searchResults(call.output)} searching={status === "inProgress"} />;
  }
  if (!call.input && !call.output) return null;
  return <ToolBox title={short} body={[call.input, call.output].filter(Boolean).join("\n\n")} />;
}

/** What a worker's action row opens to. */
export function actionDetail(item: ActionItem): ReactNode {
  switch (item.kind) {
    case "command":
      return <ShellBox command={item.command} output={item.output} status={item.status} exit={item.exitCode} />;
    case "tool":
      return callDetail(item.name, item.status, { input: item.input, output: item.output, exit: null });
    case "files": {
      const diffs = item.changes.flatMap((change) => (change.diff ? [{ path: change.path, diff: change.diff }] : []));
      if (diffs.length > 0) return <Diffs files={diffs} />;
      return <ToolBox title="Files" body={item.changes.map((change) => `${change.kind} ${change.path}`).join("\n")} />;
    }
    case "image":
      return item.path || item.prompt ? <ToolBox title="Image" body={item.path ?? item.prompt ?? ""} /> : null;
  }
}

/**
 * What a lead's tool step opens to, fetched from the thread's log when it opens: the daemon
 * keeps the call's whole input and output there, the board only its one-line detail.
 */
function LeadToolDetail({ conversationId, kind }: { conversationId: string; kind: ToolStep }) {
  const [item, setItem] = useState<ThreadItem | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let current = true;
    getThreadItem(conversationId, kind.itemId).then(
      (found) => current && setItem(found),
      (cause: unknown) => current && setError(cause instanceof Error ? cause.message : String(cause)),
    );
    return () => {
      current = false;
    };
  }, [conversationId, kind.itemId]);
  if (error) return <p className="text-destructive text-xs">{error}</p>;
  if (!item) return <span className="shimmer text-xs">Loading</span>;
  const short = toolName(kind.name);
  // An edit the CLI reported as file changes (no call input): the daemon gives their diffs, each
  // after its path.
  if (EDITS.has(short) && !item.input && item.output) return <Diffs files={fileDiffs(item.output)} />;
  // A CLI's own command gives its command line as plain text, not a call's JSON.
  const command = SHELLS.has(short) && parsedInput(item.input) === null ? item.input || kind.detail : null;
  const detail = callDetail(kind.name, kind.status, { input: item.input, output: item.output, exit: item.exit ?? kind.exit ?? null, command });
  if (detail) return <>{detail}</>;
  if (EDITS.has(short) && kind.detail) return <ToolBox title="Files" body={kind.detail} />;
  return <span className="text-muted-foreground text-xs">Nothing more was kept of this step.</span>;
}

/** Whether a lead's step has a detail to open to. */
export function leadStepOpens(step: LeadStep): boolean {
  return step.kind.type === "tool" && step.kind.status !== "inProgress";
}

/** What a lead's step opens to, in its conversation. */
export function leadStepDetail(conversationId: string | null, step: LeadStep): ReactNode {
  if (!conversationId || step.kind.type !== "tool" || !leadStepOpens(step)) return undefined;
  return <LeadToolDetail conversationId={conversationId} kind={step.kind} />;
}
