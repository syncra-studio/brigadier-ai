import { type ReactNode, useContext, useState } from "react";

import { Diffs } from "@/app/conversation/activity/Diffs";
import { Output } from "@/app/conversation/activity/Output";
import { cliRunOutput } from "@/app/conversation/activity/ownRun";
import { RawToolCallDialog } from "@/app/conversation/activity/RawToolCallDialog";
import { StoppedBy } from "@/app/conversation/activity/stopped";
import { EDITS, SHELLS } from "@/app/conversation/activity/toolSets";
import { type ActionItem, editHunks, hunkDiff, parsedInput, toolName, type ToolStep, unwrapCommand } from "@/app/conversation/activity/words";
import { CopyButton } from "@/components/shared";
import { ToolBox, ErrorState, MarkdownBlock, searchResults, WebSearch, type TranscriptItem } from "@/components/conversation";
import { Check } from "@/components/icons";
import type { ItemStatus } from "@/ipc/generated";
import { ansiRuns, stripAnsi } from "@/lib/ansi";
import { cn } from "@/lib/utils";
import { useCliCallItems } from "@/state/cliThread";

/**
 * What a step of the thread opens to (THREAD-UX-PLAN.md §3.2), the same for the lead's row and a
 * worker's: a Shell box for a command, a small diff for an edit, the results of a search, and
 * what any other tool gave back.
 */

/** A command's box: a faint card, its command up to two lines, and the end of its output. */
// Its parts keep their heights: WebKit squeezed a clamped two-line command under the output.
const SHELL = "border-foreground/16 bg-foreground/8 rounded-shell mt-1 flex flex-col overflow-hidden border *:shrink-0";
const SHELL_TEXT = "text-code font-mono whitespace-pre-wrap wrap-anywhere";

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
export function ShellBox({ command, output, status, exit, stopped = false, notes = [] }: {
  command: string;
  output: string | null;
  status: ItemStatus;
  exit: number | null;
  /** The user's Stop ended it. */
  stopped?: boolean | undefined;
  /** What Brigadier did after the command, each on a line under its output: the commit a check made. */
  notes?: string[];
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
  const printed = kept.replace(/^(?:[^\S\r\n]*\r?\n)+/, "").trimEnd();
  const end = ending(status, exit, stopped);
  const shown = unwrapCommand(command);
  return (
    <div data-slot="shell-card" className={cn(SHELL, "group/shell relative")}>
      <span className="text-foreground/70 text-label px-3 py-1">Shell</span>
      <CopyButton text={shown} tooltip="Copy command" size="xs" className="text-muted-foreground absolute end-1 top-1 opacity-0 group-hover/shell:opacity-100 focus-visible:opacity-100" />
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
        {printed && <CopyButton text={stripAnsi(printed)} tooltip="Copy output" size="xs" className="text-muted-foreground absolute end-1 top-1 opacity-0 group-hover/shell:opacity-100 focus-visible:opacity-100" />}
      </div>
      {notes.map((note) => (
        <p key={note} data-slot="shell-note" className="text-muted-foreground px-3 pt-1.5 text-sm wrap-anywhere last:pb-2">
          {note}
        </p>
      ))}
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
      {(input || text) && <RawToolCallDialog name={name} input={input} output={output} />}
    </div>
  );
}

/** A web search's query, from its call. */
function searchQuery(input: string | null): string | null {
  const query = parsedInput(input)?.["query"];
  return typeof query === "string" ? query : null;
}

/** A call's detail from what it was given and what it gave back. */
export function callDetail(name: string, status: ItemStatus, call: { input: string | null; output: string | null; exit: number | null; command?: string | null; stopped?: boolean | undefined }): ReactNode {
  const short = toolName(name);
  if (SHELLS.has(short)) {
    const given = parsedInput(call.input)?.["command"];
    const command = call.command ?? (typeof given === "string" ? given : "");
    if (command) return <ShellBox command={command} output={call.output} status={status} exit={call.exit} stopped={call.stopped} />;
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
      return <ToolBox title="Files" className="mt-1"><Output text={item.changes.map((change) => `${change.kind} ${change.path}`).join("\n")} /></ToolBox>;
    }
    case "image":
      return item.path || item.prompt ? <ToolBox title="Image" className="mt-1"><Output text={item.path ?? item.prompt ?? ""} /></ToolBox> : null;
  }
}

/**
 * A lead's call as one action, from its items: a command's, a file change's, an image's, or a
 * tool's; a shell tool still running with output printed so far (Brigadier's own `run`) is a
 * command whose output grows.
 */
export function leadAction(items: readonly TranscriptItem[], kind: ToolStep): ActionItem | null {
  const tool = items.find((item) => item.kind === "tool");
  const command = items.find((item) => item.kind === "command");
  if (tool?.kind === "tool" && command?.kind === "command" && tool.status === "inProgress" && SHELLS.has(toolName(tool.name))) {
    const given = parsedInput(tool.input)?.["command"];
    return {
      kind: "command",
      key: tool.key,
      command: typeof given === "string" ? given : (kind.detail ?? ""),
      cwd: null,
      status: "inProgress",
      exitCode: null,
      output: command.output,
      durationMs: null,
    };
  }
  const found = tool ?? command ?? items.find((item) => item.kind === "files" || item.kind === "image");
  return found && (found.kind === "tool" || found.kind === "command" || found.kind === "files" || found.kind === "image") ? found : null;
}

/** A lead's call's items, folded from the call's own item in the CLI's thread as it moves on. */
export function useLeadItems(conversationId: string, kind: ToolStep): { items: readonly TranscriptItem[]; error: null } {
  return useCliCallItems(conversationId, kind.itemId) ?? { items: [], error: null };
}

