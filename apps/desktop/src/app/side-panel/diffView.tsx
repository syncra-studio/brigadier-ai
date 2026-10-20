import { type ReactNode, Suspense } from "react";

import { preloadable } from "@/lib/preloadable";
import { cn } from "@/lib/utils";

/**
 * How a diff is drawn, the same in a thread row and in the Review tab. The engine loads on
 * demand; until it has, the lines show plain in the same type, so nothing jumps.
 */

/** A file's change: one file's patch, or (whole files loaded) both of its sides. */
export type DiffSource = { kind: "patch"; patch: string } | { kind: "sides"; path: string; before: string; after: string };

export type DiffLook = {
  /** Line numbers: only for a real patch, never for an edit rebuilt from its call. */
  numbered: boolean;
  split: boolean;
  wrap: boolean;
  /** The words a replaced line changed, marked. */
  words: boolean;
};

/** Something shown under a line: `side` "additions" is the new file's, "deletions" the old one's. */
export type DiffNote<T> = { side: "additions" | "deletions"; lineNumber: number; metadata: T };

export type DiffViewProps<T> = {
  source: DiffSource;
  look: DiffLook;
  annotations?: DiffNote<T>[];
  renderAnnotation?: (annotation: DiffNote<T>) => ReactNode;
  /** A "+" by the hovered line's number, and what it does (the Review tab's comments). */
  onGutterClick?: (line: { lineNumber: number; side: "additions" | "deletions" }) => void;
  className?: string;
};

// Once its code is in (warmed at idle), a diff draws at once: a lazy view would hold its plain
// lines for a moment more.
const engine = preloadable(() => import("@/components/views/diff").then((module) => module.default as (props: DiffViewProps<unknown>) => ReactNode));
const PierreDiff = engine.View;

/** Loads the diff engine's code ahead of the first diff. */
export const preloadDiffView = engine.preload;

export function DiffView<T>(props: DiffViewProps<T>) {
  return (
    <Suspense fallback={<PlainLines source={props.source} wrap={props.look.wrap} />}>
      <PierreDiff {...(props as DiffViewProps<unknown>)} />
    </Suspense>
  );
}

/** The change's lines as text, while the engine loads. */
function PlainLines({ source, wrap }: { source: DiffSource; wrap: boolean }) {
  const lines =
    source.kind === "sides"
      ? source.after.replace(/\n$/, "").split("\n")
      : source.patch
          .replace(/\n$/, "")
          .split("\n")
          .filter((line) => /^[ +-]/.test(line) && !/^(---|\+\+\+) /.test(line));
  return (
    <pre data-slot="diff-plain" className={cn("text-muted-foreground px-3 text-diff font-mono", wrap ? "whitespace-pre-wrap" : "whitespace-pre")}>
      {lines.join("\n")}
    </pre>
  );
}
