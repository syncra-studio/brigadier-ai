import { createContext, type ReactNode, useContext, useEffect, useState } from "react";

import { HIGHLIGHT_CHARS, languageOf, type Token } from "@/lib/languages";
import { type DiffLine, type DiffRow, wordSpans } from "@/lib/patch";
import { cn } from "@/lib/utils";

/**
 * How a diff's lines are drawn, the same in the Review tab and in a thread row's diff: each
 * line in its file's syntax colours, a changed line tinted with a bar at its start, and the
 * words a replaced line changed marked.
 */

/** Each line's colours: the old file's for deleted lines, the new one's else. */
export type LineTokens = Map<DiffLine, Token[]>;

export const TokensContext = createContext<LineTokens | null>(null);

/** Colours both sides of a file's diff, from the lines `rows` hold of each; `null` until they come. */
export function useSideTokens(path: string, rows: readonly DiffRow[] | undefined): LineTokens | null {
  // Kept with the rows they colour, so new rows show plain until their colours come.
  const [found, setFound] = useState<{ rows: readonly DiffRow[]; tokens: LineTokens } | null>(null);
  useEffect(() => {
    if (!rows) return;
    const language = languageOf(path);
    const lines = rows.flatMap((row) => (row.kind === "gap" ? (row.lines ?? []) : [row]));
    const olds = lines.filter((line) => line.kind !== "add");
    const news = lines.filter((line) => line.kind !== "del");
    const beforeText = olds.map((line) => line.text).join("\n");
    const afterText = news.map((line) => line.text).join("\n");
    if (beforeText.length + afterText.length > HIGHLIGHT_CHARS) return;
    let live = true;
    // shiki loads on demand, as for code blocks: never part of the first paint.
    import("@/lib/highlight")
      .then(({ highlight }) => Promise.all([highlight(beforeText, language), highlight(afterText, language)]))
      .then(([before, after]) => {
        if (!live || !before || !after) return;
        const tokens: LineTokens = new Map();
        olds.forEach((line, index) => line.kind === "del" && tokens.set(line, before[index] ?? []));
        news.forEach((line, index) => tokens.set(line, after[index] ?? []));
        setFound({ rows, tokens });
      })
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [path, rows]);
  return found && found.rows === rows ? found.tokens : null;
}

const MARK: Record<DiffLine["kind"], string> = { context: "", add: "bg-success/30", del: "bg-destructive/30" };

/** `text` in its colours, the parts in `spans` marked (word diffs). */
function Coloured({ tokens, spans, kind }: { tokens: readonly Token[]; spans: readonly [number, number][]; kind: DiffLine["kind"] }) {
  const parts: ReactNode[] = [];
  let offset = 0;
  tokens.forEach((token, index) => {
    const start = offset;
    const end = offset + token.content.length;
    offset = end;
    const style = token.color ? { color: token.color } : undefined;
    const italic = cn(token.italic && "italic");
    // A token is cut where a marked span starts and ends.
    const cuts = [start, ...spans.flat().filter((at) => at > start && at < end), end];
    for (let piece = 0; piece < cuts.length - 1; piece += 1) {
      const from = cuts[piece] as number;
      const to = cuts[piece + 1] as number;
      const text = token.content.slice(from - start, to - start);
      const marked = spans.some(([markFrom, markTo]) => from >= markFrom && to <= markTo);
      parts.push(
        marked ? (
          <mark key={`${index}:${piece}`} className={cn("rounded-xs", italic, MARK[kind])} style={style}>
            {text}
          </mark>
        ) : (
          <span key={`${index}:${piece}`} className={italic} style={style}>
            {text}
          </span>
        ),
      );
    }
  });
  return <>{parts}</>;
}

export function lineKey(line: DiffLine): string {
  return `${line.kind}:${line.old}:${line.new}`;
}

/** Each deleted line with the added line that took its place, for word diffs. */
export function pairChanges(rows: readonly DiffRow[]): Map<DiffLine, DiffLine> {
  const pairs = new Map<DiffLine, DiffLine>();
  let index = 0;
  while (index < rows.length) {
    const deleted: DiffLine[] = [];
    while (rows[index]?.kind === "del") deleted.push(rows[index++] as DiffLine);
    const added: DiffLine[] = [];
    while (rows[index]?.kind === "add") added.push(rows[index++] as DiffLine);
    for (let pair = 0; pair < Math.min(deleted.length, added.length); pair += 1) {
      const before = deleted[pair] as DiffLine;
      const after = added[pair] as DiffLine;
      pairs.set(before, after);
      pairs.set(after, before);
    }
    if (deleted.length === 0 && added.length === 0) index += 1;
  }
  return pairs;
}

/** A line's tint and the bar at its start, by how it changed. */
export const TINT: Record<DiffLine["kind"], string> = {
  context: "",
  add: "bg-success/10 border-success",
  del: "bg-destructive/10 border-destructive",
};

/** A line's text in its colours, with the words that differ from its partner marked (word diffs). */
export function LineText({ line, partner }: { line: DiffLine; partner: DiffLine | null }) {
  const tokens = useContext(TokensContext)?.get(line);
  const spans: [number, number][] =
    !partner || line.kind === "context"
      ? []
      : line.kind === "del"
        ? wordSpans(line.text, partner.text).before
        : wordSpans(partner.text, line.text).after;
  if (tokens && tokens.length > 0) return <Coloured tokens={tokens} spans={spans} kind={line.kind} />;
  if (spans.length === 0) return <>{line.text || " "}</>;
  return <Coloured tokens={[{ content: line.text, color: undefined, italic: false }]} spans={spans} kind={line.kind} />;
}
