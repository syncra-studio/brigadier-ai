import { type ActionItem, unwrapCommand } from "@/app/conversation/activity/words";
import type { TranscriptItem } from "@/components/transcript/transcript";

export { unwrapCommand };

/**
 * A worker's transcript as its thread shows it: replies and reasoning as they are, and each run
 * of actions between two replies together. What the actions are called is `activity/words.ts`.
 */

/** One entry of a worker's thread: a reply or row as it is, or a run of actions. */
export type ThreadEntry =
  | { kind: "item"; item: TranscriptItem }
  | { kind: "actions"; key: string; items: ActionItem[] };

/** Brigadier's own folder of tool shims, before a tool's name: `/…/gate/bin/git` → `git`. */
const GATE_PATH = /(^|[\s'"(;&|])(?:\/[\w.@+-]+(?: [\w.@+-]+)*)*?\/gate\/bin\//g;

/** git's `-c name=value` settings right after `git`. */
const GIT_SETTINGS = /(\bgit)((?:\s+-c\s+[\w.-]+=\S*)+)/g;

/**
 * Settings that change only how git prints or keeps its index, never what a command does.
 * Any other setting (`core.hooksPath`, `user.email`, an alias) stays in view: it can change
 * what the user is asked to allow.
 */
const QUIET_SETTING = /^(?:core\.splitIndex|merge\.autoStash|core\.quotePath|core\.pager|color\.[\w.-]+|advice\.[\w.-]+|pager\.[\w.-]+|column\.ui|gc\.auto|maintenance\.auto)=/i;

function withoutQuietSettings(git: string, settings: string): string {
  const kept = [...settings.matchAll(/\s+-c\s+(\S+)/g)]
    .filter((match) => !QUIET_SETTING.test(match[1] ?? ""))
    .map((match) => match[0]);
  return git + kept.join("");
}

/**
 * A command as the card shows it: what the shell wrapper runs, with Brigadier's gate folder
 * and git's display-only `-c` settings left out (`/bin/zsh -lc '/…/gate/bin/git -c
 * color.ui=never push'` → `git push`).
 */
export function shownCommand(command: string): string {
  return unwrapCommand(command).replace(GATE_PATH, "$1").replace(GIT_SETTINGS, (_, git: string, settings: string) => withoutQuietSettings(git, settings)).trim();
}

/** Tool calls the worker's CLI makes for itself, with nothing to tell. */
function hidden(item: TranscriptItem): boolean {
  return item.kind === "tool" && item.name === "ToolSearch";
}

function isAction(item: TranscriptItem): item is ActionItem {
  return item.kind === "command" || item.kind === "tool" || item.kind === "files" || item.kind === "image";
}

/**
 * A worker's transcript as its thread shows it: replies and reasoning stay, adjacent actions become one run, turn markers go. A failed or stopped turn stays.
 */
export function threadEntries(items: readonly TranscriptItem[]): ThreadEntry[] {
  const entries: ThreadEntry[] = [];
  for (const item of items) {
    if (hidden(item) || item.kind === "turnStarted") continue;
    if (item.kind === "turnCompleted" && item.status === "completed") continue;
    if (isAction(item)) {
      const last = entries.at(-1);
      if (last?.kind === "actions") last.items.push(item);
      else entries.push({ kind: "actions", key: item.key, items: [item] });
      continue;
    }
    entries.push({ kind: "item", item });
  }
  return entries;
}
