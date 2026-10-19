import { previewActive } from "@/app/conversation/previewStatus";
import type { BackgroundRun, Preview, Task } from "@/ipc/generated";

/**
 * The context card's Background rows (Q91): one row per process the session keeps running, with
 * its state in a word. A worker's second copy of an app that already has a row (a smoke test of
 * the running app, a check against it) and a headless browser pointed at it are a state of that
 * row, never rows of their own.
 *
 * A row stays while it still needs the user (Q94): one of the thread's that failed stays until
 * its Restart, its dismiss or the thread starts the same process again; one that finished on its
 * own stays until the next message is sent; one that was stopped leaves at once.
 */

export type BackgroundState = "building" | "starting" | "running" | "paused" | "smoke testing" | "checking" | "failed" | "finished";

/** What takes an ended row off the card: the user's dismiss or Restart (`cleared`, by id), and the newest message sent. */
export type BackgroundLeft = { cleared: readonly string[]; lastSentMs: number | null };

export type BackgroundRow =
  | { type: "preview"; id: string; startedAtMs: number; preview: Preview; state: BackgroundState; worker: string | null; with: Preview[] }
  | { type: "run"; id: string; startedAtMs: number; run: BackgroundRun; state: BackgroundState };

/** Whether a row's process has ended (it failed or finished): its row is still, and its time stops. */
export const rowEnded = (state: BackgroundState) => state === "failed" || state === "finished";

/** How an ended preview stands on its row: a clean exit finished, any other exit failed; one Brigadier stopped leaves (null). */
export function endedPreviewState(preview: Preview): "failed" | "finished" | null {
  if (preview.state.type !== "exited") return null;
  return preview.state.code === 0 ? "finished" : "failed";
}

/** How an ended run stands on its row: "exit 0" finished, any other end failed (an interruption too); one stopped leaves (null). */
export function endedRunState(run: BackgroundRun): "failed" | "finished" | null {
  switch (run.state.type) {
    case "finished": return run.state.status === "exit 0" ? "finished" : "failed";
    case "interrupted": return "failed";
    default: return null;
  }
}

/** Leading `NAME=value` assignments, as a shell reads them before the command. */
const ASSIGNMENTS = /^(?:\s*[A-Za-z_][A-Za-z0-9_]*=(?:"[^"]*"|'[^']*'|\S*)\s+)+/;

/** What runs, without the variables set for this copy of it (`PORT=3000 node src/server.js`). */
export function appCommand(command: string): string {
  return command.replace(ASSIGNMENTS, "").trim();
}

/** A headless browser driven at a page: a check of the app serving it. */
const BROWSER = /(chrome|chromium|headless|playwright|puppeteer)/i;

/** The port a preview serves on, or points a browser at. */
function portOf(preview: Preview): string | null {
  return (
    preview.url?.match(/:(\d+)/)?.[1] ??
    preview.command.match(/\bPORT=["']?(\d+)/)?.[1] ??
    preview.command.match(/localhost:(\d+)|127\.0\.0\.1:(\d+)/)?.slice(1).find(Boolean) ??
    null
  );
}

/** A run's state from what it runs: a build builds, a test or lint checks. */
export function runState(command: string): BackgroundState {
  const words = appCommand(command);
  if (/\b(test|tests|check|lint|typecheck|clippy|vitest|jest|pytest|playwright)\b/i.test(words)) return "checking";
  if (/\b(build|compile|bundle|tsc)\b/i.test(words)) return "building";
  return "running";
}

/** Whether `a` started after `b`. */
const later = (a: { startedAtMs: number; id: string }, b: { startedAtMs: number; id: string }) =>
  a.startedAtMs > b.startedAtMs || (a.startedAtMs === b.startedAtMs && a.id.localeCompare(b.id) > 0);

/**
 * The rows for the session's previews that still run and its long commands, newest first. Of a
 * group of copies of one app, the thread's own (else the oldest) is the row; the others are its
 * state while they run: "smoke testing" for a worker that only looks at the running app or a
 * browser at it, "checking" for any other worker's copy. The thread's that ended stay until
 * `left` takes them off (Q94); a worker's that ended is its worker's to deal with.
 */
export function backgroundRows(
  previews: readonly Preview[],
  runs: readonly BackgroundRun[],
  tasks: Record<string, Task> | null,
  workerName: (task: Task) => string | null,
  left: BackgroundLeft = { cleared: [], lastSentMs: null },
): BackgroundRow[] {
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
  const lookOnly = (preview: Preview) => {
    const task = preview.taskId ? tasks?.[preview.taskId] : undefined;
    return BROWSER.test(preview.command) || /smoke/i.test(preview.name) || task?.liveLook === true;
  };
  const rows: BackgroundRow[] = leads.map(({ lead, with: copies }) => {
    const task = lead.taskId ? tasks?.[lead.taskId] : undefined;
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
    return { type: "preview", id: lead.id, startedAtMs: lead.startedAtMs, preview: lead, state, worker: task ? workerName(task) : null, with: copies };
  });
  for (const run of runs) {
    if (run.state.type === "running") rows.push({ type: "run", id: run.id, startedAtMs: run.startedAtMs, run, state: runState(run.command) });
  }
  const stays = (id: string, state: "failed" | "finished" | null, endedAtMs: number): state is "failed" | "finished" =>
    state !== null && !left.cleared.includes(id) && (state === "failed" || left.lastSentMs === null || left.lastSentMs <= endedAtMs);
  for (const preview of previews) {
    const state = endedPreviewState(preview);
    if (preview.taskId || !stays(preview.id, state, preview.endedAtMs ?? preview.startedAtMs)) continue;
    // The thread started the same process again: the new one is its row.
    const key = `${preview.kind}:${appCommand(preview.command)}`;
    if (previews.some((other) => !other.taskId && later(other, preview) && `${other.kind}:${appCommand(other.command)}` === key)) continue;
    rows.push({ type: "preview", id: preview.id, startedAtMs: preview.startedAtMs, preview, state, worker: null, with: [] });
  }
  for (const run of runs) {
    const state = endedRunState(run);
    if (!stays(run.id, state, run.endedAtMs ?? run.startedAtMs)) continue;
    if (runs.some((other) => later(other, run) && appCommand(other.command) === appCommand(run.command))) continue;
    rows.push({ type: "run", id: run.id, startedAtMs: run.startedAtMs, run, state });
  }
  return rows.toSorted((a, b) => b.startedAtMs - a.startedAtMs || b.id.localeCompare(a.id));
}
