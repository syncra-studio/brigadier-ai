/**
 * Inline code in an answer that names a file of the session's checkout (`src/cli.ts:64`) is a
 * file chip. A file counts only when it exists: in the checkout's file list, else asked of the
 * daemon by its exact path (the list stops at its cap and misses files made since it was read).
 * What is known is read again whenever the checkout may have changed, so a chip is never false for
 * long and a real path never stays plain; until the new answer comes the old one stays shown, so
 * the chips of a finished answer hold still while a later turn works.
 */
import { useEffect, useSyncExternalStore } from "react";

import { request } from "@/ipc/client";
import { listFiles } from "@/state/actions";

/** A path as written in inline code, with the line it points at. */
export type CodePath = { path: string; line: number | null; end: number | null };

/** `src/cli.ts`, `./a/b.rs:12`, `notes.md:3-9`, `/abs/file.py`: a file name with an extension, no spaces. */
const PATH = /^(\.{0,2}\/)?((?:[\w@.+-]+\/)*[\w@+-][\w@.+-]*\.[A-Za-z][\w]{0,9})(?::(\d+)(?:-(\d+))?)?$/;

/** Extensions a bare file name (no folder) must have to read as one. */
const EXTENSIONS = new Set(
  ("md mdx txt json jsonc yml yaml toml ini cfg conf env lock csv tsv xml html htm css scss sass less svg png jpg jpeg gif webp pdf " +
    "ts tsx mts cts js jsx mjs cjs vue svelte astro rs py pyi go java kt kts swift m mm c h cc cpp hpp cs fs rb php pl lua dart " +
    "ex exs erl zig nix tf proto graphql gql sql sh bash zsh fish ps1 bat gradle mod sum ipynb r jl scala clj hs ml elm sol wasm").split(" "),
);

/** The file inline code names, if it reads as one; `null` for code that isn't a path. */
export function codePath(text: string): CodePath | null {
  const found = PATH.exec(text.trim());
  if (!found) return null;
  const [, lead = "", rest = "", line, end] = found;
  // A dotted name with no folder (`Math.round`) reads as a path only with a file extension.
  if (!lead && !rest.includes("/") && !EXTENSIONS.has(rest.slice(rest.lastIndexOf(".") + 1).toLowerCase())) return null;
  return { path: `${lead === "./" ? "" : lead}${rest}`, line: line ? Number(line) : null, end: end ? Number(end) : null };
}

/** The absolute path `path` names under `root`. */
export function underRoot(root: string, path: string): string {
  return path.startsWith("/") ? path : `${root.replace(/\/$/, "")}/${path}`;
}

type Known = {
  /** What the checkout changes with: what is known holds for this. */
  version: string;
  /** The checkout's files, relative; `null` until read. */
  files: Set<string> | null;
  truncated: boolean;
  listing: boolean;
  /** Paths to look up once the list is read. */
  waiting: [relative: string | null, absolute: string][];
  /** Absolute paths asked of the daemon, and whether each exists. */
  asked: Map<string, boolean | "asking">;
  /** What was known for the checkout's version before this one: shown until this one knows. */
  before: Known | null;
};

const known = new Map<string, Known>();
const listeners = new Set<() => void>();
let generation = 0;
const changed = () => {
  generation += 1;
  for (const listener of listeners) listener();
};

function entry(conversationId: string, version: string): Known {
  const current = known.get(conversationId);
  if (current && current.version === version) return current;
  // The last version read through is shown meanwhile (one still being read knows nothing yet);
  // older ones go.
  const before = current ? (current.files !== null ? current : current.before) : null;
  if (before) before.before = null;
  const fresh: Known = { version, files: null, truncated: false, listing: false, waiting: [], asked: new Map(), before };
  known.set(conversationId, fresh);
  return fresh;
}

/** Whether `absolute` (`relative` under the checkout) exists: true, false, or `null` while unknown. */
function lookup(state: Known, relative: string | null, absolute: string): boolean | null {
  if (relative !== null && state.files?.has(relative)) return true;
  const asked = state.asked.get(absolute);
  if (asked === undefined || asked === "asking") return null;
  return asked;
}

/** Reads what isn't known yet: the file list once, then the exact path when the list doesn't have it. */
function learn(conversationId: string, version: string, relative: string | null, absolute: string): void {
  const state = entry(conversationId, version);
  if (state.files === null) {
    state.waiting.push([relative, absolute]);
    if (state.listing) return;
    state.listing = true;
    listFiles(conversationId)
      .then(({ files, truncated }) => {
        state.files = new Set(files);
        state.truncated = truncated;
      })
      .catch(() => {
        state.files = new Set();
        state.truncated = true;
      })
      .finally(() => {
        state.listing = false;
        const waiting = state.waiting.splice(0);
        for (const [path, whole] of waiting) ask(conversationId, state, path, whole);
        changed();
      });
    return;
  }
  ask(conversationId, state, relative, absolute);
}

/** Asks the daemon whether a path the list doesn't have exists. */
function ask(conversationId: string, state: Known, relative: string | null, absolute: string): void {
  if (relative !== null && state.files?.has(relative)) return;
  if (state.asked.has(absolute)) return;
  state.asked.set(absolute, "asking");
  request({ method: "resolveFileLink", conversationId, link: absolute, path: absolute })
    .then((found) => state.asked.set(absolute, found.exists))
    .catch(() => state.asked.set(absolute, false))
    .finally(changed);
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/**
 * Whether the file inline code names exists in the session's checkout: `null` while unknown (or
 * with no session), so the code shows plain until it is known. While a new version of the
 * checkout is read, the last version's answer stands.
 */
export function useCodePathExists(conversationId: string | null, root: string | null, version: string, path: CodePath | null): boolean | null {
  useSyncExternalStore(subscribe, () => generation);
  // Only the checkout's own files.
  const absolute = path && root && (!path.path.startsWith("/") || path.path.startsWith(`${root.replace(/\/$/, "")}/`)) ? underRoot(root, path.path) : null;
  const relative = path && root && !path.path.startsWith("/") ? path.path.replace(/^\.\//, "") : null;
  const state = conversationId ? known.get(conversationId) : undefined;
  const current = state && state.version === version ? state : undefined;
  const result = current && absolute ? lookup(current, relative, absolute) : null;
  // Not read for this version yet: what the last version read through knew.
  const latest = current ?? state;
  const previous = latest && (latest === current || latest.files === null) ? latest.before : latest;
  const shown = result ?? (previous && absolute ? lookup(previous, relative, absolute) : null);
  useEffect(() => {
    if (!conversationId || !absolute || result !== null) return;
    learn(conversationId, version, relative, absolute);
  }, [conversationId, version, relative, absolute, result]);
  return shown;
}
