/**
 * A command's output with its terminal colours: the text in runs, each with the colour and
 * weight its SGR codes gave it, in the terminal palette's colours (`--ansi-*`). Every other escape sequence (cursor moves, titles, links) is
 * left out, so the text reads as the terminal printed it.
 */

/** The 8 base colours, as the terminal's palette names them (`--ansi-*` tokens). */
const NAMES = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"] as const;

export type AnsiStyle = { color?: string; background?: string; bold?: boolean; dim?: boolean; italic?: boolean; underline?: boolean };
export type AnsiRun = { text: string; style: AnsiStyle };

const token = (index: number, bright: boolean) => `var(--ansi-${bright ? "bright-" : ""}${NAMES[index]})`;

/** A 256-colour code: its first 16 are the palette's; the rest (a cube and greys) keep the default colour. */
function color256(code: number): string | undefined {
  return code >= 0 && code < 16 ? token(code % 8, code >= 8) : undefined;
}

/** The style after one SGR sequence's codes. */
function apply(style: AnsiStyle, codes: number[]): AnsiStyle {
  const next = { ...style };
  for (let at = 0; at < codes.length; at++) {
    const code = codes[at] ?? 0;
    if (code === 0) {
      for (const key of Object.keys(next)) delete next[key as keyof AnsiStyle];
    } else if (code === 1) next.bold = true;
    else if (code === 2) next.dim = true;
    else if (code === 3) next.italic = true;
    else if (code === 4) next.underline = true;
    else if (code === 22) {
      delete next.bold;
      delete next.dim;
    } else if (code === 23) delete next.italic;
    else if (code === 24) delete next.underline;
    else if (code >= 30 && code <= 37) next.color = token(code - 30, false);
    else if (code >= 90 && code <= 97) next.color = token(code - 90, true);
    else if (code === 39) delete next.color;
    else if (code >= 40 && code <= 47) next.background = token(code - 40, false);
    else if (code >= 100 && code <= 107) next.background = token(code - 100, true);
    else if (code === 49) delete next.background;
    else if (code === 38 || code === 48) {
      const key = code === 38 ? "color" : "background";
      let value: string | undefined;
      if (codes[at + 1] === 5) {
        value = color256(codes[at + 2] ?? -1);
        at += 2;
      } else if (codes[at + 1] === 2) {
        // A true colour is no palette colour: the default stays.
        at += 4;
      }
      if (value) next[key] = value;
    }
  }
  return next;
}

// CSI sequences (SGR among them), OSC sequences ending in BEL or ST, and lone two-byte escapes.
// eslint-disable-next-line no-control-regex
const ESCAPE = /\u001b\[([0-9;?]*)([@-~])|\u001b\][^\u0007\u001b]*(?:\u0007|\u001b\\)|\u001b[@-Z\\-_]/g;

/** The output as runs of styled text; output with no escapes is one plain run. */
export function ansiRuns(text: string): AnsiRun[] {
  const runs: AnsiRun[] = [];
  let style: AnsiStyle = {};
  let from = 0;
  const push = (to: number) => {
    if (to <= from) return;
    const piece = text.slice(from, to);
    const last = runs.at(-1);
    if (last && last.style === style) last.text += piece;
    else runs.push({ text: piece, style });
  };
  for (const match of text.matchAll(ESCAPE)) {
    push(match.index);
    from = match.index + match[0].length;
    if (match[2] === "m") {
      const codes = (match[1] ?? "").split(";").map((part) => (part === "" ? 0 : Number(part)));
      style = apply(style, codes);
    }
  }
  push(text.length);
  return runs;
}

/** The output's text with every escape left out. */
export function stripAnsi(text: string): string {
  return text.replace(ESCAPE, "");
}
