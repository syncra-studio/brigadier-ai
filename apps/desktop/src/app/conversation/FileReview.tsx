import { type FC, type RefObject, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";

import { DiffView, type DiffNote } from "@/app/conversation/diffView";
import { StepCounter } from "@/app/conversation/StepCounter";
import { shortcutKeys } from "@/app/shortcutCatalog";
import { Button, Kbd } from "@/components/controls";
import { ChevronRight } from "@/components/icons";
import type { PendingFile } from "@/ipc/generated";
import { request } from "@/ipc/client";
import { changeLine, reviewPatch } from "@/lib/reviewPatch";
import { keepChanges, undoChanges, usePendingPaths } from "@/state/pendingChanges";
import { openFileTab } from "@/state/sessionTabs";
import { useApp } from "@/state/store";

/**
 * A file tab's review of what agents changed in its file, while any of it waits for the user's
 * Keep or Undo: the file's inline diff with Undo and Keep on each change, and in the tab's bar
 * where the change and the file are among the rest (⌃ 1 of 2 ⌄, ‹ 1 of 3 Files ›), Undo File
 * and Keep File. Once its file is done, while other files still wait, the bar offers "Keep all
 * changes" and "Review next file". ⌘Y keeps the change in view, ⌘N undoes it, ⌘⏎ keeps the
 * file, ⌥L and ⌥H open the next and previous file.
 */

export type FileReviewState = {
  conversationId: string;
  path: string;
  /** Every file waiting for review, in path order. */
  paths: string[];
  /** This file's changes, once read; null while it has none to review. */
  file: PendingFile | null;
  /** The bar shows: the file has changes to review, or had them and others still wait. */
  shown: boolean;
  current: number;
  setCurrent: (index: number) => void;
  /** Brings the change after (1) or before (-1) the one in view, or `from`, into view. */
  step: (by: 1 | -1, from?: number) => void;
  /** The tab's diff, where its changes are found. */
  box: RefObject<HTMLDivElement | null>;
  busy: boolean;
  act: (what: "keep" | "undo", hunk?: number) => void;
  keepAll: () => void;
  /** Opens the next (1) or previous (-1) file waiting for review. */
  move: (step: 1 | -1) => void;
};

/** Typing in a field never reaches the review's keys. */
function typing(target: EventTarget | null): boolean {
  return target instanceof HTMLElement && (target.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName));
}

export function useFileReview(conversationId: string, path: string, again: number, active: boolean): FileReviewState {
  const paths = usePendingPaths(conversationId);
  const pending = paths.includes(path);
  // Reviewed in this tab: its bar stays, done, while other files still wait.
  const [reviewed, setReviewed] = useState(pending);
  if (pending && !reviewed) setReviewed(true);
  if (reviewed && paths.length === 0) setReviewed(false);
  const [read, setRead] = useState<{ key: string; file: PendingFile | null } | null>(null);
  const [reload, setReload] = useState(0);
  const key = `${conversationId}:${path}:${again}:${reload}`;
  useEffect(() => {
    if (!pending) return;
    let live = true;
    request({ method: "getPendingChanges", conversationId, path })
      .then(({ pending: { files } }) => live && setRead({ key, file: files.find((file) => file.path === path) ?? null }))
      .catch(() => live && setRead({ key, file: null }));
    return () => {
      live = false;
    };
  }, [pending, conversationId, path, key]);
  // The last read stands until the next one lands, so the diff doesn't blink after each Keep.
  const file = pending && read?.file?.path === path ? read.file : null;
  const [current, setCurrent] = useState(0);
  const count = file?.hunks.length ?? 0;
  const at = Math.min(current, Math.max(count - 1, 0));
  const [busy, setBusy] = useState(false);
  const run = (done: Promise<boolean>) => {
    setBusy(true);
    void done.finally(() => {
      setBusy(false);
      setReload((n) => n + 1);
    });
  };
  const act = (what: "keep" | "undo", hunk?: number) => {
    if (busy || !file) return;
    const id = hunk === undefined ? undefined : file.hunks[hunk]?.id;
    if (hunk !== undefined && id === undefined) return;
    const ids = id === undefined ? undefined : [id];
    run(what === "keep" ? keepChanges(conversationId, path, ids) : undoChanges(conversationId, path, ids));
  };
  const keepAll = () => !busy && run(keepChanges(conversationId));
  const move = (step: 1 | -1) => {
    const others = paths.filter((each) => each !== path);
    if (others.length === 0) return;
    const index = paths.indexOf(path);
    const next =
      index >= 0
        ? paths[(index + step + paths.length) % paths.length]
        : step === 1
          ? (others.find((each) => each > path) ?? others[0])
          : (others.toReversed().find((each) => each < path) ?? others.at(-1));
    if (next) openFileTab(conversationId, next);
  };
  const box = useRef<HTMLDivElement>(null);
  const step = (by: 1 | -1, from = at) => {
    if (count === 0) return;
    const next = (from + by + count) % count;
    setCurrent(next);
    changeBar(box.current, next)?.scrollIntoView({ block: "center" });
  };
  const shown = pending || (reviewed && paths.length > 0);
  const mac = useApp((s) => s.info?.platform === "macos");
  useEffect(() => {
    if (!active || !shown) return;
    const onKey = (event: KeyboardEvent) => {
      if (typing(event.target) || event.shiftKey) return;
      const mod = mac ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey;
      let done = true;
      if (mod && !event.altKey && event.code === "KeyY" && file) act("keep", at);
      else if (mod && !event.altKey && event.code === "KeyN" && file) act("undo", at);
      else if (mod && !event.altKey && event.key === "Enter" && file) act("keep");
      else if (event.altKey && !event.metaKey && !event.ctrlKey && event.code === "KeyL") move(1);
      else if (event.altKey && !event.metaKey && !event.ctrlKey && event.code === "KeyH") move(-1);
      else done = false;
      if (done) event.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });
  return { conversationId, path, paths, file, shown, current: at, setCurrent, step, box, busy, act, keepAll, move };
}

/** A change's own bar in this tab's diff. */
function changeBar(box: HTMLElement | null, index: number): HTMLElement | null {
  return box?.querySelector<HTMLElement>(`[data-slot="review-change"][data-index="${index}"]`) ?? null;
}

/**
 * The change the keys act on once the user scrolls: the current one while its bar is in view,
 * else the first in view.
 */
function changeInView(box: HTMLElement, current: number): number | null {
  const { top, bottom } = box.getBoundingClientRect();
  const seen = (bar: HTMLElement) => {
    const at = bar.getBoundingClientRect().top;
    return at >= top && at < bottom;
  };
  const now = changeBar(box, current);
  if (now && seen(now)) return current;
  const bars = [...box.querySelectorAll<HTMLElement>("[data-slot=review-change]")];
  const bar = bars.find(seen) ?? bars.find((each) => each.getBoundingClientRect().top >= top) ?? bars.at(-1);
  return bar ? Number(bar.dataset.index) : null;
}

/** A line of the file as it is now, in the diff's lines. */
function fileLine(box: HTMLElement, line: number): Element | null {
  for (const host of box.querySelectorAll("diffs-container")) {
    const found = host.shadowRoot?.querySelector(`[data-content] > [data-line="${line}"]:not([data-line-type=change-deletion])`);
    if (found) return found;
  }
  return null;
}

/** The review's part of the file tab's bar, at its right. */
export const FileReviewBar: FC<{ review: FileReviewState }> = ({ review }) => {
  const { file, paths, path, busy, current } = review;
  const mac = useApp((s) => s.info?.platform === "macos");
  if (!file) {
    return (
      <span data-slot="review-bar" className="flex shrink-0 items-center gap-1">
        <Button size="xs" variant="ghost" disabled={busy} onClick={review.keepAll}>
          Keep all changes
        </Button>
        <Button size="xs" onClick={() => review.move(1)}>
          Review next file
          <ChevronRight className="size-icon-xs" />
        </Button>
      </span>
    );
  }
  const count = file.hunks.length;
  return (
    <span data-slot="review-bar" className="flex shrink-0 items-center gap-2">
      {count > 0 && <StepCounter data-slot="review-changes" noun="change" vertical at={current} count={count} onStep={(by) => review.step(by)} />}
      <span className="bg-border h-3 w-px" />
      <StepCounter
        data-slot="review-files"
        noun="file"
        at={paths.indexOf(path)}
        count={paths.length}
        unit={paths.length === 1 ? "File" : "Files"}
        onStep={review.move}
      />
      <Button
        size="xs"
        variant="outline"
        disabled={busy || file.held !== undefined}
        title={file.held}
        onClick={() => review.act("undo")}
      >
        Undo File
      </Button>
      <Button size="xs" disabled={busy} onClick={() => review.act("keep")}>
        Keep File <Kbd>{shortcutKeys("review-keep-file", mac)}</Kbd>
      </Button>
    </span>
  );
};

/** One change's Undo and Keep, over the right end of its first line. */
const ChangeBar: FC<{ review: FileReviewState; index: number }> = ({ review, index }) => {
  const mac = useApp((s) => s.info?.platform === "macos");
  const file = review.file!;
  const now = index === review.current;
  return (
    <div data-slot="review-change" data-index={index} data-current={now || undefined} className="relative h-0 font-sans">
      <div className="absolute -top-(--text-diff--line-height) end-3 flex gap-1">
        <span className="bg-muted rounded-control flex h-control-xs items-center px-1">
          <StepCounter data-slot="review-change-steps" noun="change" vertical at={index} count={file.hunks.length} onStep={(by) => review.step(by, index)} />
        </span>
        <Button
          size="xs"
          variant="outline"
          className="bg-background"
          disabled={review.busy || file.held !== undefined}
          title={file.held}
          onClick={() => {
            review.setCurrent(index);
            review.act("undo", index);
          }}
        >
          Undo {now && <Kbd>{shortcutKeys("review-undo", mac)}</Kbd>}
        </Button>
        <Button
          size="xs"
          className="text-background bg-success hover:bg-success"
          disabled={review.busy}
          onClick={() => {
            review.setCurrent(index);
            review.act("keep", index);
          }}
        >
          Keep {now && <Kbd>{shortcutKeys("review-keep", mac)}</Kbd>}
        </Button>
      </div>
    </div>
  );
};

/**
 * The file's inline diff: its old lines above its new ones, each change with its own Undo and
 * Keep. Scrolling makes the change in view the one the keys act on; a line asked for (a search
 * result, a link) is brought into view.
 */
export const FileReviewBody: FC<{ review: FileReviewState; line: number | null; reveal: number }> = ({ review, line, reveal }) => {
  const file = review.file!;
  const { box, setCurrent, current } = review;
  // Asked again (`reveal`), the same line is brought into view again.
  const asked = useMemo(() => (line === null ? null : { line, reveal }), [line, reveal]);
  useLayoutEffect(() => {
    const element = box.current;
    if (asked === null || !element) return;
    let tries = 0;
    let frame = 0;
    // The diff draws its lines after it loads.
    const find = () => {
      const found = fileLine(element, asked.line);
      if (found) found.scrollIntoView({ block: "center" });
      else if (tries++ < 120) frame = requestAnimationFrame(find);
    };
    find();
    return () => cancelAnimationFrame(frame);
  }, [box, asked]);
  if (!file.text) {
    return <p className="text-muted-foreground p-4 text-sm">This change can't be shown: keep or undo the whole file</p>;
  }
  if (file.hunks.length === 0) {
    return <p className="text-muted-foreground p-4 text-sm">Only the file's mode changed: keep or undo the whole file</p>;
  }
  const notes: DiffNote<number>[] = file.hunks.map((hunk, index) => ({ ...changeLine(hunk), metadata: index }));
  return (
    <div
      ref={box}
      data-slot="file-review"
      className="min-h-0 flex-1 overflow-auto py-2"
      onScroll={(event) => {
        const index = changeInView(event.currentTarget, current);
        if (index !== null) setCurrent(index);
      }}
    >
      <DiffView<number>
        source={{ kind: "patch", patch: reviewPatch(file.path, file.base ?? null, file.current ?? null, file.hunks) }}
        look={{ numbered: true, split: false, wrap: false, words: true }}
        annotations={notes}
        renderAnnotation={(note) => <ChangeBar review={review} index={note.metadata} />}
      />
    </div>
  );
};
