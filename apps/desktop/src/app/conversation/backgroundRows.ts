import { taskShown } from "@/app/conversation/backgroundTasks";
import { unwrapCommand } from "@/app/conversation/activity/words";
import { previewActive } from "@/app/conversation/previewStatus";
import type { BackgroundTask, FileEntry, Preview } from "@/ipc/generated";

/**
 * The context card's Background rows (Q91, S12): one row per thing the session has going in the
 * background, with its state in a word: a preview it keeps running (`brigadier preview`), and its
 * CLI's own background tasks (Claude Code's `/tasks`, Codex's `/ps`). A worker's second copy of an app
 * that already has a row (a smoke test of the running app, a check against it) and a headless
 * browser pointed at it are a state of that row, never rows of their own.
 *
 * A row leaves as soon as its thing ends, finished, failed or stopped: the thread's fold keeps
 * the history.
 */

export type BackgroundState = "building" | "starting" | "running" | "paused" | "smoke testing" | "checking" | "waiting" | "merging" | "done";

export type BackgroundRow =
  | { type: "preview"; id: string; startedAtMs: number; preview: Preview; state: BackgroundState; worker: string | null; with: Preview[] }
  /** One of the CLI's own background tasks (Claude Code's `/tasks`, Codex's `/ps`). */
  | { type: "task"; id: string; startedAtMs: number; task: BackgroundTask; state: BackgroundState };

/** Leading `NAME=value` assignments, as a shell reads them before the command. */
const ASSIGNMENTS = /^(?:\s*[A-Za-z_][A-Za-z0-9_]*=(?:"[^"]*"|'[^']*'|\S*)\s+)+/;

/** What runs, without the variables set for this copy of it (`PORT=3000 node src/server.js`). */
export function appCommand(command: string): string {
  return command.replace(ASSIGNMENTS, "").trim();
}

/** A headless browser driven at a page: a check of the app serving it. */
const BROWSER = /(chrome|chromium|headless|playwright|puppeteer)/i;

/** A copy of an app that only looks at it: a browser at it, or a smoke test. */
const lookOnly = (preview: Preview) => BROWSER.test(preview.command) || /smoke/i.test(preview.name);

/** The port a preview serves on, or points a browser at. */
function portOf(preview: Preview): string | null {
  return (
    preview.url?.match(/:(\d+)/)?.[1] ??
    preview.command.match(/\bPORT=["']?(\d+)/)?.[1] ??
    preview.command.match(/localhost:(\d+)|127\.0\.0\.1:(\d+)/)?.slice(1).find(Boolean) ??
    null
  );
}

/** A wait invoked as a command, including after `start &&`, never words inside an argument. */
function runsBrigadierWait(command: string): boolean {
  const tokens = unwrapCommand(command).match(/(?:'[^']*'|"(?:\\.|[^"\\])*"|\\[^\n]|[^\s;&|()'"\\])+|[;&|()\n]/g) ?? [];
  let start = true;
  for (const [index, token] of tokens.entries()) {
    if (/^[;&|()\n]$/.test(token)) {
      start = true;
    } else if (start) {
      if (/^[A-Za-z_][A-Za-z0-9_]*=/.test(token)) continue;
      if (/^(?:\S*\/)?brigadier$/.test(token) && tokens[index + 1] === "wait") return true;
      start = false;
    }
  }
  return false;
}

/** Claude labels a Bash task with its description; only its originating call has the command. */
function taskCommand(task: BackgroundTask, entries: readonly FileEntry[]): string | null {
  if (task.kind === "terminal") return task.label;
  if (task.kind !== "shell" || task.itemId === null) return null;
  const call = entries.find(({ event, parent }) => !parent && event.type === "toolCall" && event.itemId === task.itemId)?.event;
  if (call?.type !== "toolCall") return null;
  try {
    const input = JSON.parse(call.input ?? "null") as { command?: unknown } | null;
    return typeof input?.command === "string" ? input.command : null;
  } catch {
    return null;
  }
}

/** Running CLI tasks, excluding the session's worker wait already covered by the workers card. */
export function taskRows(tasks: readonly BackgroundTask[], entries: readonly FileEntry[]): BackgroundRow[] {
  return tasks.filter((task) => taskShown(task) && !runsBrigadierWait(taskCommand(task, entries) ?? "")).map((task) => ({
    type: "task", id: `task:${task.id}`, startedAtMs: task.startedAtMs, task, state: "running",
  }));
}

/**
 * The rows for the session's previews that still run and its events (`eventRows`), newest
 * first. Of a group of copies of one app, the thread's own (else the
 * oldest) is the row; the others are its state while they run: "smoke testing" for a worker
 * that only looks at the running app or a browser at it, "checking" for any other worker's copy.
 */
export function backgroundRows(previews: readonly Preview[], workerName: (taskId: string) => string | null): BackgroundRow[] {
  const active = previews.filter((preview) => previewActive(preview.state)).toSorted((a, b) => a.startedAtMs - b.startedAtMs || a.id.localeCompare(b.id));
  const browsers = active.filter((preview) => BROWSER.test(preview.command));
  const apps = active.filter((preview) => !browsers.includes(preview));
  const groups = new Map<string, Preview[]>();
  for (const preview of apps) {
    const key = `${preview.kind}:${appCommand(preview.command)}`;
    groups.set(key, [...(groups.get(key) ?? []), preview]);
  }
  const leads = [...groups.values()].map((group) => {
    const lead = group.find((preview) => !preview.taskId) ?? group[0]!;
    return { lead, with: group.filter((preview) => preview !== lead) };
  });
  // A browser goes with the app on its port, else with the newest web app; alone it is a row.
  for (const browser of browsers) {
    const port = portOf(browser);
    const served = leads.find(({ lead, with: copies }) => port !== null && [lead, ...copies].some((preview) => preview !== browser && portOf(preview) === port))
      ?? leads.findLast(({ lead }) => lead.kind === "web");
    if (served) served.with.push(browser);
    else leads.push({ lead: browser, with: [] });
  }
  const rows: BackgroundRow[] = leads.map(({ lead, with: copies }) => {
    const state: BackgroundState =
      lead.state.type === "starting"
        ? lead.building ? "building" : "starting"
        : lead.state.type === "paused"
          ? "paused"
          : copies.some(lookOnly)
            ? "smoke testing"
            : copies.length > 0
              ? "checking"
              : "running";
    return { type: "preview", id: lead.id, startedAtMs: lead.startedAtMs, preview: lead, state, worker: lead.taskId ? workerName(lead.taskId) : null, with: copies };
  });
  return rows.toSorted((a, b) => b.startedAtMs - a.startedAtMs || b.id.localeCompare(a.id));
}
