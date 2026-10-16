import { Check, Copy } from "@openai/apps-sdk-ui/components/Icon";
import { type ReactNode, useContext, useEffect, useState } from "react";

import type { LeadStep } from "@/app/conversation/activity/group";
import { InlineDiff } from "@/app/conversation/activity/InlineDiff";
import { cliRunOutput, OWN_RUNS, ownRunOutput } from "@/app/conversation/activity/ownRun";
import { StoppedBy } from "@/app/conversation/activity/stopped";
import { type ActionItem, editHunks, fileDiffs, hunkDiff, parsedInput, toolName, type ToolStep, unwrapCommand } from "@/app/conversation/activity/words";
import { ErrorState } from "@/components/assistant-ui/elements/error-state";
import { MarkdownBlock } from "@/components/assistant-ui/thread";
import { Dialog, DialogContent, DialogTitle, DialogTrigger } from "@/components/ui/dialog";
import { searchResults, WebSearch } from "@/components/assistant-ui/elements/web-search";
import { TooltipIconButton } from "@/components/assistant-ui/tooltip-icon-button";
import { useCopyToClipboard } from "@/hooks/use-copy-to-clipboard";
import type { ItemStatus, ThreadItem } from "@/ipc/generated";
import { ansiRuns, stripAnsi } from "@/lib/ansi";
import { cn } from "@/lib/utils";
import { getThreadItem } from "@/state/actions";

/**
 * What a step of the thread opens to (THREAD-UX-PLAN.md §3.2), the same for the lead's row and a
 * worker's: a Shell box for a command, a small diff for an edit, the results of a search, and
 * what any other tool gave back.
 */

const BOX = "border-border bg-code-surface rounded-control mt-1 flex flex-col border text-sm";

/** A command's box: a faint card, its command up to two lines, and the end of its output. */
// Its parts keep their heights: WebKit squeezed a clamped two-line command under the output.
const SHELL = "border-foreground/16 bg-foreground/8 rounded-shell mt-1 flex flex-col overflow-hidden border *:shrink-0";
const SHELL_TEXT = "text-code font-mono whitespace-pre-wrap wrap-anywhere";
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

/** How a command ended, in the Shell box's last line; nothing while it runs. */
function ending(status: ItemStatus, exit: number | null, stopped: boolean): { text: string; ok?: boolean } | null {
  if (stopped) return { text: "Stopped" };
  if (status === "inProgress") return null;
  if (status === "declined") return { text: "Declined" };
  if (exit !== null && exit !== 0) return { text: `Exit code ${exit}` };
  if (status === "failed") return { text: "Failed" };
  return { text: "Success", ok: true };
}

/** What the daemon's clip leaves at the end of output it cut: `… [12345 more bytes]`. */
const CLIPPED = /… \[(\d+) more bytes\]$/;

/** Output whose end was cut: the part kept, and how much more there was. */
export function clippedOutput(output: string): { kept: string; more: number | null } {
  const found = CLIPPED.exec(output);
  return found ? { kept: output.slice(0, found.index), more: Number(found[1]) } : { kept: output, more: null };
}

function sizeWords(bytes: number): string {
  if (bytes < 1024) return `${bytes} bytes`;
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

/** Output as the terminal coloured it. */
function Colored({ text }: { text: string }) {
  return (
    <>
      {ansiRuns(text).map((run, index) => {
        const { color, background, bold, dim, italic, underline } = run.style;
        if (!color && !background && !bold && !dim && !italic && !underline) return run.text;
        return (
          <span
            key={index}
            style={{ color, backgroundColor: background }}
            className={cn(bold && "font-semibold", dim && "opacity-60", italic && "italic", underline && "underline")}
          >
            {run.text}
          </span>
        );
      })}
    </>
  );
}

/**
 * A command's box: "Shell", `$ command` (two lines, all of it on a click) and everything it printed
 * in colour, newest line in view, then how it ended. Output the daemon cut short says so.
 */
export function ShellBox({ command, output, status, exit, stopped = false }: {
  command: string;
  output: string | null;
  status: ItemStatus;
  exit: number | null;
  /** The user's Stop ended it. */
  stopped?: boolean;
}) {
  const [whole, setWhole] = useState(false);
  // A command still running when the user's Stop ended its thread was stopped.
  const stoppedBy = useContext(StoppedBy);
  stopped ||= status === "inProgress" && stoppedBy !== null;
  // A CLI's failed command reply starts with "Exit code N", which the box's ending says: read, not shown.
  const cli = cliRunOutput(output);
  if (cli.exit !== null && (exit === null || exit === cli.exit)) {
    output = cli.output;
    exit = cli.exit;
  }
  const { kept, more } = clippedOutput(output?.trimEnd() ?? "");
  const printed = kept.trimEnd();
  const end = ending(status, exit, stopped);
  const shown = unwrapCommand(command);
  return (
    <div data-slot="shell-card" className={cn(SHELL, "group/shell relative")}>
      <span className="text-foreground/70 text-label px-3 py-1">Shell</span>
      <CopyButton text={shown} label="Copy command" className="end-1 top-1" />
      {/* Two lines, then all of it on a click. */}
      <pre
        data-slot="shell-command"
        data-whole={whole || undefined}
        {...(!whole && {
          role: "button",
          tabIndex: 0,
          onClick: () => setWhole(true),
          onKeyDown: (event: React.KeyboardEvent) => {
            if (event.key === "Enter" || event.key === " ") {
              event.preventDefault();
              setWhole(true);
            }
          },
        })}
        className={cn("text-code text-foreground px-3 pt-2 font-mono whitespace-pre-wrap wrap-anywhere", !whole && "line-clamp-2 cursor-pointer")}
      >
        <span className="text-muted-foreground">$ </span>
        {shown}
      </pre>
      <div className="relative min-w-0">
        {/* Its newest line in view, the older ones fading out above it. */}
        {(printed || status !== "inProgress") && (
          <div data-slot="shell-output" className="max-h-shell-output scroll-edge-fade flex flex-col-reverse overflow-y-auto px-3 py-2 [--spacing-scroll-fade-bottom:0] [--spacing-scroll-fade-top:var(--spacing-shell-fade)]">
            <pre className={cn(SHELL_TEXT, "text-foreground/70 font-medium")}>{printed ? <Colored text={printed} /> : "No output"}</pre>
          </div>
        )}
        {printed && <CopyButton text={stripAnsi(printed)} label="Copy output" className="end-1 top-1" />}
      </div>
      {(end || more !== null) && (
        <div className="text-muted-foreground flex items-center gap-2 px-3 py-1.5 text-sm">
          {more !== null && <span data-slot="shell-clipped" className="min-w-0 truncate">Only the start was kept: {sizeWords(more)} more not shown</span>}
          {end && (
            <span data-slot="shell-ending" className="ms-auto flex shrink-0 items-center gap-1">
              {end.ok && <Check aria-hidden className="size-icon-sm" />}
              {end.text}
            </span>
          )}
        </div>
      )}
    </div>
  );
}

/** Copies a command or its output; shown on hover of the box. */
function CopyButton({ text, label, className }: { text: string; label: string; className: string }) {
  const { isCopied, copyToClipboard } = useCopyToClipboard();
  return (
    <TooltipIconButton
      tooltip={isCopied ? "Copied" : label}
      size="xs"
      className={cn(
        "text-muted-foreground absolute opacity-0 group-hover/shell:opacity-100 focus-visible:opacity-100",
        className,
      )}
      onClick={() => copyToClipboard(text)}
    >
      {isCopied ? <Check /> : <Copy />}
    </TooltipIconButton>
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

/** A tool's output as structured data: pretty-printed, or `null` when it is plain text. */
export function structuredOutput(output: string): string | null {
  const text = output.trim();
  if (!text.startsWith("{") && !text.startsWith("[")) return null;
  try {
    return JSON.stringify(JSON.parse(text), null, 2);
  } catch {
    return null;
  }
}

/**
 * What a tool gave back, its arguments left to the raw view: structured data as a JSON code
 * block, text up to 192px then scrolling, a failure in the error callout, or "Tool returned no
 * content"; under it, "Show raw tool call output" opens what it was given and gave back as is.
 */
export function ToolResult({ name, status, input, output }: { name: string; status: ItemStatus; input: string | null; output: string | null }) {
  const text = output?.trim() ?? "";
  const json = text ? structuredOutput(text) : null;
  return (
    <div data-slot="tool-result" className="flex min-w-0 flex-col gap-1 pt-1">
      {status === "failed" ? (
        <ErrorState summary={`${name} failed.`} details={text || null} />
      ) : !text ? (
        <p data-slot="tool-empty" className="text-muted-foreground text-sm">Tool returned no content</p>
      ) : json ? (
        <div data-slot="tool-json" className="max-h-48 min-w-0 overflow-auto [&_.aui-md>:first-child]:mt-0 [&_.aui-md>:last-child]:mb-0">
          <MarkdownBlock text={`\`\`\`json\n${json}\n\`\`\``} />
        </div>
      ) : (
        <pre data-slot="tool-text" className="text-code text-foreground/80 max-h-48 overflow-auto font-mono whitespace-pre-wrap wrap-anywhere">
          {text}
        </pre>
      )}
      {(input || text) && (
        <Dialog>
          <DialogTrigger asChild>
            <button type="button" data-slot="tool-raw" className="text-muted-foreground hover:text-foreground self-start text-xs transition-colors">
              Show raw tool call output
            </button>
          </DialogTrigger>
          <DialogContent aria-describedby={undefined} className="max-w-thread flex max-h-full flex-col gap-3">
            <DialogTitle>{name}</DialogTitle>
            <div className="flex min-h-0 flex-col gap-3 overflow-auto">
              {input && <ToolBox title="Input" body={input} />}
              {text && <ToolBox title="Output" body={output ?? ""} />}
            </div>
          </DialogContent>
        </Dialog>
      )}
    </div>
  );
}

/**
 * Several files' changes, each its own card; an edit's several places in one file are one card,
 * "⋯" between them.
 */
function Diffs({ files }: { files: readonly { path: string; diff: string }[] }) {
  const byPath = new Map<string, string[]>();
  for (const file of files) byPath.set(file.path, [...(byPath.get(file.path) ?? []), file.diff]);
  return (
    <div className="flex flex-col gap-1">
      {[...byPath].map(([path, diffs]) => (
        <InlineDiff key={path} path={path} diff={diffs.join("\n@@\n")} />
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
    if (command && OWN_RUNS.has(short)) {
      const own = ownRunOutput(call.output);
      return <ShellBox command={command} output={own.output} status={status} exit={call.exit ?? own.exit} />;
    }
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
        <ErrorState summary="The search failed." details={call.output || null} />
      </div>
    );
  }
  if (query !== null) {
    // A search with no results kept has nothing to open: its row says what it searched.
    const results = searchResults(call.output);
    return results.length > 0 ? <WebSearch className="pt-1" query={query} results={results} /> : null;
  }
  if (!call.input && !call.output) return null;
  return <ToolResult name={short} status={status} input={call.input} output={call.output} />;
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
  // A search with no results kept: nothing more to show than its row.
  if (short === "WebSearch" || short === "web_search") return null;
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
