import { getSharedHighlighter } from "@pierre/diffs";
import { type DiffLineAnnotation, type FileDiffOptions, MultiFileDiff, PatchDiff } from "@pierre/diffs/react";
import { type CSSProperties, type ReactNode, useMemo } from "react";

import type { DiffSource, DiffViewProps } from "@/app/conversation/diffView";
import { COMMON_LANGUAGES, WARM_PIECES, yieldToPage } from "@/lib/highlight";

/**
 * The diff engine itself, loaded on demand through `DiffView` so it and its highlighter never
 * weigh on the first paint. Lines in the dark theme's syntax colours, changed lines tinted with a
 * bar at their start, the words a replaced line changed marked.
 */

/** The diff's type and spacing, from the thread's tokens. */
const LOOK = {
  "--diffs-font-family": "var(--font-mono)",
  "--diffs-header-font-family": "var(--font-sans)",
  "--diffs-font-size": "var(--text-diff)",
  "--diffs-line-height": "var(--text-diff--line-height)",
  "--diffs-tab-size": "2",
} as CSSProperties;

/**
 * The engine's highlighter: shiki's WebAssembly one, several times faster in WebKit than its
 * JavaScript one (see `@/lib/highlight`).
 */
const HIGHLIGHTER = "shiki-wasm";

/**
 * Loads the diff engine's highlighter, its theme and the common grammars ahead of the first diff,
 * TypeScript's patterns compiled on a sample, a piece at a time (see `WARM_SAMPLE`).
 */
export async function warmDiffs(): Promise<void> {
  const shiki = await getSharedHighlighter({ themes: ["pierre-dark"], langs: [...COMMON_LANGUAGES], preferredHighlighter: HIGHLIGHTER });
  for (const lang of ["typescript", "tsx"]) {
    for (const piece of WARM_PIECES) {
      shiki.codeToTokens(piece, { lang, theme: "pierre-dark" });
      await yieldToPage();
    }
  }
}

/** The theme paints its own near-black; the diff sits on the thread's background instead. */
const ON_THREAD = "pre { --diffs-bg: var(--background) !important; background-color: var(--background) !important; }";

export default function PierreDiff<T>({ source, look, annotations, renderAnnotation, onGutterClick, className }: DiffViewProps<T>) {
  const options = useMemo<FileDiffOptions<T, undefined>>(
    () => ({
      theme: "pierre-dark",
      themeType: "dark",
      preferredHighlighter: HIGHLIGHTER,
      disableFileHeader: true,
      disableLineNumbers: !look.numbered,
      diffStyle: look.split ? "split" : "unified",
      overflow: look.wrap ? "wrap" : "scroll",
      lineDiffType: look.words ? "word" : "none",
      hunkSeparators: look.numbered ? "line-info" : "simple",
      expandUnchanged: false,
      unsafeCSS: ON_THREAD,
      ...(onGutterClick && {
        enableGutterUtility: true,
        onGutterUtilityClick: (range) => onGutterClick({ lineNumber: range.end, side: range.endSide ?? range.side ?? "additions" }),
      }),
    }),
    [look.numbered, look.split, look.wrap, look.words, onGutterClick],
  );
  const shared = {
    options,
    style: LOOK,
    ...(className !== undefined && { className }),
    ...(annotations && { lineAnnotations: annotations as unknown as DiffLineAnnotation<T>[] }),
    ...(renderAnnotation && { renderAnnotation: renderAnnotation as unknown as (annotation: DiffLineAnnotation<T>) => ReactNode }),
  };
  return sides(source) ? (
    <MultiFileDiff<T>
      {...shared}
      oldFile={{ name: source.path, contents: source.before }}
      newFile={{ name: source.path, contents: source.after }}
    />
  ) : (
    <PatchDiff<T> {...shared} patch={source.patch} />
  );
}

function sides(source: DiffSource): source is Extract<DiffSource, { kind: "sides" }> {
  return source.kind === "sides";
}
