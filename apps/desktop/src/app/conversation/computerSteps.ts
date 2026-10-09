import type { ThreadEntry } from "@/components/transcript/activity";
import type { ComputerAction } from "@/ipc/generated";
import type { ActionItem } from "@/app/conversation/activity/words";

/*
 * A worker's computer timeline (COMPUTER-USE-PLAN.md §5, Phase 3): its computer calls folded
 * out of its activity into one disclosure, and its action log in plain words, batch by batch.
 */

const TOOLS = "apps|launch|observe|act|zoom";
/** A `computer` MCP server's tool, as Claude (`mcp__computer__act`) and Codex (`computer/act`, `computer.act`) name it. */
const MCP_TOOL = new RegExp(`(^|__)computer(__|[./])(${TOOLS})$`);
/** The same tools from a shell: `brigadierd computer act …`, `"$BRIGADIER_COMPUTER_CLI" computer observe …`. */
const CLI = new RegExp(`(brigadierd|BRIGADIER_COMPUTER_CLI\\}?"?)\\s+computer\\s+(${TOOLS})\\b`);

/** Whether a worker's action is one of its computer calls. */
export function isComputerItem(item: ActionItem): boolean {
  if (item.kind === "tool") return MCP_TOOL.test(item.name);
  if (item.kind === "command") return CLI.test(item.command);
  return false;
}

/** The worker's entries without its computer calls, and those calls, in order. */
export function foldComputer(entries: readonly ThreadEntry[]): { entries: ThreadEntry[]; calls: ActionItem[] } {
  const calls: ActionItem[] = [];
  const kept: ThreadEntry[] = [];
  for (const entry of entries) {
    if (entry.kind !== "actions") {
      kept.push(entry);
      continue;
    }
    const items = entry.items.filter((item) => {
      if (!isComputerItem(item)) return true;
      calls.push(item);
      return false;
    });
    if (items.length === entry.items.length) kept.push(entry);
    else if (items.length > 0) kept.push({ ...entry, items });
  }
  return { entries: kept, calls };
}

/** One `act` call's actions, with the marked screenshot of where they aimed. */
export type Batch = { key: string; actions: ComputerAction[]; image: string | null; atMs: number };

/** The actions grouped by batch, oldest first. An action from before batches were kept is its own. */
export function batchesOf(actions: readonly ComputerAction[]): Batch[] {
  const batches: Batch[] = [];
  for (const action of actions) {
    const last = batches.at(-1);
    if (last && action.batch && last.key === action.batch) {
      last.actions.push(action);
      last.image ??= action.image;
      continue;
    }
    batches.push({ key: action.batch || `${action.atMs}:${batches.length}`, actions: [action], image: action.image, atMs: action.atMs });
  }
  return batches;
}

/** `button "Save"` → `“Save”`; a role alone → `the text field`. */
function targetWords(target: string | null): string | null {
  if (!target) return null;
  const labelled = /^[\w-]+ "(.*)"$/.exec(target);
  if (labelled) return `“${labelled[1]}”`;
  if (/^[\w-]+$/.test(target)) return `the ${target.replaceAll("-", " ").replace("textfield", "text field")}`;
  return `“${target}”`;
}

type RecordedAction = { do?: string; count?: number; button?: string; action?: string };

/** The action as the engine recorded it (its `do`, a click's count and button…), when readable. */
function recorded(action: ComputerAction): RecordedAction {
  try {
    const record = JSON.parse(action.record) as { action?: RecordedAction };
    return record.action ?? {};
  } catch {
    return {};
  }
}

/** What the action did, in words: "Clicked “Save”", "Typed into the text field", "Chose File › Save". */
export function actionWords(action: ComputerAction): string {
  const target = targetWords(action.target);
  const on = (verb: string, fallback: string) => (target ? `${verb} ${target}` : fallback);
  switch (action.kind) {
    case "click": {
      const r = recorded(action);
      const verb = r.button === "right" ? "Right-clicked" : (r.count ?? 1) >= 2 ? "Double-clicked" : "Clicked";
      return on(verb, `${verb} a point`);
    }
    case "set_value":
      return on("Set", "Set a value");
    case "type":
      return target ? `Typed into ${target}` : "Typed text";
    case "key":
      return action.target ? `Pressed ${action.target}` : "Pressed a key";
    case "scroll":
      return on("Scrolled", "Scrolled");
    case "drag":
      return on("Dragged", "Dragged");
    case "perform": {
      const what = recorded(action).action;
      return target ? `Used ${what ?? "an action"} on ${target}` : "Used an element's action";
    }
    case "menu":
      return action.target ? `Chose ${action.target}` : "Chose a menu item";
    case "select":
      return on("Selected text in", "Selected text");
    case "wait":
      return "Waited for a change";
    default:
      return on(action.kind, action.kind);
  }
}

/** What came of the action, in a few words, and whether it went wrong. */
export function outcomeWords(action: ComputerAction): { text: string; failed: boolean } {
  if (action.status === "skipped") return { text: "skipped", failed: false };
  if (action.status === "failed") {
    if (action.effect === "background_unavailable" || action.error === "background_unavailable") {
      return { text: "couldn't do it in the background", failed: true };
    }
    return { text: action.detail ? `failed: ${action.detail}` : "failed", failed: true };
  }
  switch (action.effect) {
    case "confirmed":
      return { text: "worked", failed: false };
    case "no_change":
      return { text: "nothing changed", failed: false };
    default:
      return { text: "done", failed: false };
  }
}

/** How it was delivered and how long that took: the details line. */
export function routeWords(action: ComputerAction): string {
  const route = (() => {
    switch (action.rung) {
      case "element":
        return "Directly, through the app's accessibility";
      case "background":
      case "background_activated":
        return "In the background";
      case "foreground":
        return "Brought the window to the front while you were away";
      default:
        return null;
    }
  })();
  const where = action.appWindow ? `${action.app ? `${action.app}, ` : ""}“${action.appWindow}”` : action.app;
  const time = action.status === "skipped" ? null : `${Math.round(action.dispatchMs)} ms`;
  return [route, where, time].filter(Boolean).join(" · ");
}

/** The disclosure's line: "Used the computer · 14 steps in Target Range and TextEdit". */
export function summaryWords(actions: readonly ComputerAction[], live: boolean): string {
  const last = actions.at(-1);
  if (live && last) return `Using the computer · ${actionWords(last)}${last.app ? ` in ${last.app}` : ""}`;
  if (live) return "Using the computer";
  const apps = [...new Set(actions.map((action) => action.app).filter(Boolean))];
  const steps = `${actions.length} ${actions.length === 1 ? "step" : "steps"}`;
  const where = apps.length === 0 ? "" : apps.length <= 2 ? ` in ${apps.join(" and ")}` : ` in ${apps.length} apps`;
  return actions.length === 0 ? "Used the computer" : `Used the computer · ${steps}${where}`;
}

/** Where the timeline is: the batch shown, and whether it plays. */
export type Playback = { index: number; playing: boolean };

/** The timeline's keys: ← and → step through the batches, Home and End jump, Space plays. */
export function playbackKey(state: Playback, key: string, count: number): Playback | null {
  const last = Math.max(0, count - 1);
  switch (key) {
    case "ArrowLeft":
      return { index: Math.max(0, state.index - 1), playing: false };
    case "ArrowRight":
      return { index: Math.min(last, state.index + 1), playing: false };
    case "Home":
      return { index: 0, playing: false };
    case "End":
      return { index: last, playing: false };
    case " ":
      return playToggle(state, count);
    default:
      return null;
  }
}

/** Play from where it is (from the start once at the end), or pause. */
export function playToggle(state: Playback, count: number): Playback {
  if (state.playing) return { ...state, playing: false };
  return { index: state.index >= count - 1 ? 0 : state.index, playing: count > 1 };
}

/** One step of playback: the next batch, stopping at the last. */
export function playStep(state: Playback, count: number): Playback {
  const index = Math.min(count - 1, state.index + 1);
  return { index, playing: state.playing && index < count - 1 };
}

/** How long playback shows each batch. */
export const PLAY_MS = 1200;
