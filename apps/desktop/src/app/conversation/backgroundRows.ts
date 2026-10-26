import { persistentApp } from "@/app/conversation/persistentApps";
import { taskEnd, taskShown } from "@/app/conversation/backgroundTasks";
import { brigadierCalls } from "@/app/conversation/activity/words";
import { previewActive } from "@/app/conversation/previewStatus";
import type { BackgroundTask, FileEntry, Preview, WorkerRow } from "@/ipc/generated";

/**
 * The context card's Background rows (Q91, S12): one row per thing the session has going in the
 * background, with its state in a word: a preview it keeps running (`brigadier preview`), its
 * CLI's own background tasks (Claude Code's `/tasks`, Codex's `/ps`) and its workers' CLIs'. A worker's second copy of an app
 * that already has a row (a smoke test of the running app, a check against it) and a headless
 * browser pointed at it are a state of that row, never rows of their own.
 *
 * A preview's row leaves as soon as it ends. Ended tasks are retained by durable History, never live Background rows.
 */

export type BackgroundState =
  | "building" | "starting" | "running" | "paused" | "smoke testing" | "checking" | "waiting" | "merging" | "done"
  | "finished" | "failed" | "stopped";

export type BackgroundRow =
  | { type: "preview"; id: string; startedAtMs: number; preview: Preview; state: BackgroundState; worker: string | null; with: Preview[] }
  /**
   * One of the CLI's own background tasks (Claude Code's `/tasks`, Codex's `/ps`), or of a worker's
   * CLI (`worker`: its id and name).
   */
  | { type: "task"; id: string; startedAtMs: number; task: BackgroundTask; command?: string | null; state: BackgroundState; worker: { id: string; name: string | null } | null };

/** A CLI task's row state: running, or how it ended. */
function taskState(task: BackgroundTask): BackgroundState {
  return taskEnd(task)?.word ?? "running";
}

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
  return runsBrigadier(command, "wait");
}

/**
 * Whether `command` runs `brigadier <sub>` as a command of its own (after `start &&` too), never
 * words inside an argument (`core::manager::cli_jobs::runs_brigadier` reads it the same).
 */
export function runsBrigadier(command: string, sub: string): boolean {
  return brigadierCalls(command).includes(sub);
}

/**
 * Claude labels a Bash task with its description; only its originating call has the command:
 * a `command` item, as Claude's Bash always is (D38), or a tool call's `command` argument.
 */
export function taskCommand(task: BackgroundTask, entries: readonly FileEntry[]): string | null {
  if (task.kind !== "shell" && task.kind !== "terminal") return null;
  const fallback = task.kind === "terminal" ? task.label : null;
  if (task.itemId === null) return fallback;
  const call = entries.find(({ event, parent }) => !parent && (event.type === "command" || event.type === "toolCall") && event.itemId === task.itemId)?.event;
  if (call?.type === "command") return call.command || fallback;
  if (call?.type !== "toolCall") return fallback;
  try {
    const input = JSON.parse(call.input ?? "null") as { command?: unknown } | null;
    return typeof input?.command === "string" ? input.command || fallback : fallback;
  } catch {
    return fallback;
  }
}

/**
 * Running CLI tasks (`taskShown`), excluding the session's worker
 * wait already covered by the workers card.
 */
export function taskRows(tasks: readonly BackgroundTask[], entries: readonly FileEntry[], now: number): BackgroundRow[] {
  return tasks.filter((task) => taskShown(task, now) && !runsBrigadierWait(taskCommand(task, entries) ?? "")).map((task) => ({
    type: "task", id: `task:${task.id}`, command: taskCommand(task, entries), startedAtMs: task.startedAtMs, task, state: taskState(task), worker: null,
  }));
}

/**
 * The workers' CLIs' background tasks, by the same rules as the session's own: no row for a
 * subagent or a `brigadier wait`, each named by its worker.
 */
export function workerTaskRows(workers: readonly WorkerRow[], workerName: (taskId: string) => string | null, now: number): BackgroundRow[] {
  return workers.flatMap((worker) => (worker.background ?? [])
    .filter(({ task, command }) => taskShown(task, now) && task.kind !== "agent"
      && !runsBrigadierWait(command ?? (task.kind === "terminal" ? task.label : "")))
    .map(({ task, command }): BackgroundRow => ({
      type: "task", id: `task:${worker.id}:${task.id}`, command, startedAtMs: task.startedAtMs, task, state: taskState(task),
      worker: { id: worker.id, name: workerName(worker.id) },
    })));
}

/** Only proven app invocations belong beside previews; labels never establish command evidence. */
export function appTaskRows(tasks: readonly BackgroundTask[], entries: readonly FileEntry[], now: number): BackgroundRow[] {
  return taskRows(tasks, entries, now).filter((row) => row.type === "task" && persistentApp(taskCommand(row.task, entries) ?? ""));
}

export function workerAppTaskRows(workers: readonly WorkerRow[], workerName: (id: string) => string | null, now: number): BackgroundRow[] {
  const apps = workers.map((worker) => ({ ...worker, background: (worker.background ?? []).filter(({ task, command }) =>
    (task.kind === "shell" || task.kind === "terminal") && persistentApp(command ?? (task.kind === "terminal" ? task.label : ""))) }));
  return workerTaskRows(apps, workerName, now);
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
