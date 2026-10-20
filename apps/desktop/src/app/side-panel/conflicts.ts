/**
 * A conflicted file's text as git marks it, read into its plain parts and its conflicts, and
 * written back once the user picks a side: the conflict tab's Accept Current / Incoming / Both,
 * and the merge editor's result.
 */

/** One conflict: the checkout's lines, the common version's (when marked), the incoming lines. */
export type Conflict = {
  type: "conflict";
  current: string[];
  base: string[] | null;
  incoming: string[];
  /** What follows `<<<<<<<`, `|||||||` and `>>>>>>>`: HEAD, the common version, the incoming branch. */
  currentLabel: string;
  baseLabel: string;
  incomingLabel: string;
  /** How many characters each marker has: 7 unless `.gitattributes` sets `conflict-marker-size`. */
  size: number;
};

export type Segment = { type: "common"; lines: string[] } | Conflict;

/** The parts of a marked text, in order. A text with no complete conflict is one common part. */
export function parseConflicts(text: string): Segment[] {
  const lines = splitLines(text);
  const segments: Segment[] = [];
  let common: string[] = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i]!;
    const size = markerSize(line, "<");
    const conflict = size > 0 ? readConflict(lines, i, size) : null;
    if (!conflict) {
      common.push(line);
      i += 1;
      continue;
    }
    if (common.length > 0) segments.push({ type: "common", lines: common });
    common = [];
    segments.push(conflict.conflict);
    i = conflict.next;
  }
  if (common.length > 0) segments.push({ type: "common", lines: common });
  return segments;
}

/** A conflict starting at `start`, and the line after it; null when its markers don't close. */
function readConflict(lines: string[], start: number, size: number): { conflict: Conflict; next: number } | null {
  const current: string[] = [];
  const incoming: string[] = [];
  let base: string[] | null = null;
  let baseLabel = "";
  let part: "current" | "base" | "incoming" = "current";
  for (let i = start + 1; i < lines.length; i += 1) {
    const line = lines[i]!;
    if (part === "current" && markerSize(line, "|") === size) {
      part = "base";
      base = [];
      baseLabel = label(line, size);
    } else if (part !== "incoming" && markerSize(line, "=") === size && line.trimEnd().length === size) {
      part = "incoming";
    } else if (part === "incoming" && markerSize(line, ">") === size) {
      return {
        conflict: {
          type: "conflict",
          current,
          base,
          incoming,
          currentLabel: label(lines[start]!, size),
          baseLabel,
          incomingLabel: label(line, size),
          size,
        },
        next: i + 1,
      };
    } else if (markerSize(line, "<") === size) {
      return null;
    } else {
      (part === "current" ? current : part === "base" ? base! : incoming).push(line);
    }
  }
  return null;
}

/** The length of the run of `mark` that starts `line` when it is a marker (7 or more, then a space or the line's end), else 0. */
function markerSize(line: string, mark: string): number {
  let size = 0;
  while (line[size] === mark) size += 1;
  if (size < 7) return 0;
  const after = line[size];
  return after === undefined || after === " " || after === "\n" || after === "\r" ? size : 0;
}

function label(marker: string, size: number): string {
  return marker.slice(size).trim();
}

/** The text's lines, each with its own line ending kept, so a file writes back as it was. */
function splitLines(text: string): string[] {
  return text.match(/[^\n]*\n|[^\n]+$/g) ?? [];
}

/** Each part's place among the conflicts (-1 for a common part). */
export function conflictIndices(segments: readonly Segment[]): number[] {
  let seen = -1;
  return segments.map((segment) => (segment.type === "conflict" ? (seen += 1) : -1));
}

export function conflictCount(segments: readonly Segment[]): number {
  return segments.filter((segment) => segment.type === "conflict").length;
}

/** Which side the conflict tab keeps of one conflict. */
export type Choice = "current" | "incoming" | "both";

/** `text` with its `index`th conflict replaced by the side chosen. */
export function acceptConflict(text: string, index: number, choice: Choice): string {
  let seen = -1;
  return parseConflicts(text)
    .flatMap((segment) => {
      if (segment.type === "common") return segment.lines;
      seen += 1;
      if (seen !== index) return markedLines(segment);
      return choice === "current" ? segment.current
        : choice === "incoming" ? segment.incoming
          : [...segment.current, ...ended(segment.current, segment.incoming)];
    })
    .join("");
}

/** A conflict written out again as git marked it. */
function markedLines(conflict: Conflict): string[] {
  const marker = (mark: string, text = "") => `${mark.repeat(conflict.size)} ${text}`.trimEnd() + "\n";
  return [
    marker("<", conflict.currentLabel),
    ...conflict.current,
    ...(conflict.base ? [marker("|", conflict.baseLabel), ...conflict.base] : []),
    marker("="),
    ...conflict.incoming,
    marker(">", conflict.incomingLabel),
  ];
}

/** `next` after `lines`, with a line break between them if the last of `lines` has none. */
function ended(lines: string[], next: string[]): string[] {
  const last = lines[lines.length - 1];
  return last !== undefined && !last.endsWith("\n") && next.length > 0 ? [`\n${next[0]}`, ...next.slice(1)] : next;
}

/**
 * The merge editor's take on one conflict: which sides are in the result and in what order,
 * and whether the user has dealt with it (accepted a side, or ignored it).
 */
export type Pick = { order: Side[]; handled: boolean };
export type Side = "current" | "incoming";

export const UNTOUCHED: Pick = { order: [], handled: false };

/** One conflict's lines in the result: the sides picked in order, else the common version. */
export function pickedLines(conflict: Conflict, pick: Pick): string[] {
  if (pick.order.length === 0) return conflict.base ?? [];
  let lines: string[] = [];
  for (const side of pick.order) {
    const next = side === "current" ? conflict.current : conflict.incoming;
    lines = lines.length === 0 ? [...next] : [...lines, ...ended(lines, next)];
  }
  return lines;
}

/** The merge editor's result: every common part, and each conflict as it was picked. */
export function mergedResult(segments: readonly Segment[], picks: readonly Pick[]): string {
  let seen = -1;
  return segments
    .flatMap((segment) => {
      if (segment.type === "common") return segment.lines;
      seen += 1;
      return pickedLines(segment, picks[seen] ?? UNTOUCHED);
    })
    .join("");
}

/** Adds or takes out a side, as the merge editor's Accept, Accept Combination and Remove do. */
export function toggleSide(pick: Pick, side: Side, alsoOther = false): Pick {
  const other: Side = side === "current" ? "incoming" : "current";
  if (pick.order.includes(side)) {
    const order = pick.order.filter((entry) => entry !== side);
    return { order, handled: order.length > 0 || pick.handled };
  }
  const order: Side[] = alsoOther ? [side, other] : [...pick.order, side];
  return { order: order.filter((entry, index) => order.indexOf(entry) === index), handled: true };
}

/**
 * The merge editor's parts: the working file as it is now (so lines the user already edited
 * stay), each conflict taking the common version from git's three-way marked text `merged`.
 * Null when the two no longer pair up conflict for conflict (a conflict was already accepted in
 * place, or edited away), since each conflict's common version is then unknown.
 */
export function editorSegments(text: string | null, merged: string | null): Segment[] | null {
  if (text === null || merged === null) return null;
  const working = parseConflicts(text);
  const bases = parseConflicts(merged).flatMap((segment) => (segment.type === "conflict" ? [segment.base] : []));
  const count = conflictCount(working);
  if (count === 0 || count !== bases.length) return null;
  let seen = -1;
  return working.map((segment) => {
    if (segment.type === "common") return segment;
    seen += 1;
    return { ...segment, base: bases[seen] ?? null };
  });
}
