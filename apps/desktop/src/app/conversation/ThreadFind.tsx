import { ChevronLeft, ChevronRight, Search, X } from "@openai/apps-sdk-ui/components/Icon";
import { useCallback, useContext, useEffect, useLayoutEffect, useRef, useState, type FC, type KeyboardEvent as ReactKeyboardEvent } from "react";

import { ThreadHistoryContext } from "@/components/assistant-ui/message-rail";
import type { ThreadOverlayProps } from "@/components/assistant-ui/thread";
import { TooltipIconButton } from "@/components/assistant-ui/tooltip-icon-button";
import { useApp } from "@/state/store";

/*
 * Find in the chat (⌘F with the chat in front): a bar at the top right of the thread finds text
 * in what the thread shows, case aside: the user's messages and the answers, work rows' labels,
 * the lines between steps. Every match is washed yellow and the one gone to orange, through the
 * CSS Custom Highlight API, so the thread's DOM is never touched. Enter, ⌘G and › go to the next
 * one, ⇧Enter, ⇧⌘G and ‹ the one before, Esc closes.
 *
 * What it searches is what is mounted: turns folded behind "N earlier messages" are unfolded for
 * it, but a closed work group or fold holds no rows until opened, and pages still in the database
 * are not loaded for it. Diffs draw in their own shadow tree and are not searched either.
 */

/** Counting stops here: "250+ results". */
export const MAX_RESULTS = 250;
/** The highlights the matches are drawn with (globals.css). */
export const MATCHES = "thread-find";
export const ACTIVE_MATCH = "thread-find-active";
/** On the thread while it holds marks: its text is selectable (globals.css). */
export const FINDING = "data-thread-finding";
/** On a button holding the match gone to, whose text WebKit never marks. */
export const ACTIVE_BUTTON = "data-thread-find-active";
/** How soon after the thread changes the search runs again, while the bar is open. */
const RERUN_MS = 150;
/** A match gone to clears the bar and the composer by this much. */
const CLEAR_PX = 16;

/** What is never searched, with all it holds. */
const SKIP = [
  "script",
  "style",
  "textarea",
  "input",
  "select",
  '[contenteditable]:not([contenteditable="false"])',
  "[hidden]",
  "[data-thread-find-skip]",
  "[data-thread-scroll-footer]",
  ".sr-only",
].join(", ");

/** Text in different ones of these never runs together into one match. */
const BLOCK = "p, li, pre, h1, h2, h3, h4, h5, h6, td, th, tr, blockquote, dt, dd, div, button, summary, label";

/** "2 / 7 results", "1 / 1 result", "3 / 250+ results", "0 results". */
export function findLabel(active: number, count: number, more: boolean): string {
  if (count === 0) return "0 results";
  if (more) return `${active + 1} / ${MAX_RESULTS}+ results`;
  return `${active + 1} / ${count} ${count === 1 ? "result" : "results"}`;
}

/** Lower case, code unit by code unit where lowering a character would change its length. */
function lower(text: string): string {
  const all = text.toLowerCase();
  if (all.length === text.length) return all;
  let out = "";
  for (const unit of text.split("")) {
    const lowered = unit.toLowerCase();
    out += lowered.length === 1 ? lowered : unit;
  }
  return out;
}

/** Whether an element shows no text at all: not laid out (`display: none` here or above). */
function unseen(element: Element): boolean {
  if (typeof element.checkVisibility !== "function" || element.checkVisibility()) return false;
  // `display: contents` has no box of its own, but what it holds shows.
  return getComputedStyle(element).display !== "contents";
}

export type FindResult = { ranges: Range[]; more: boolean };

const NONE: FindResult = { ranges: [], more: false };

/** The matches of `query` in the text `root` shows, top to bottom, the first `MAX_RESULTS` of them. */
export function findIn(root: Element, query: string): FindResult {
  if (query === "") return NONE;
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_ELEMENT | NodeFilter.SHOW_TEXT, {
    acceptNode: (node) => {
      if (node instanceof Text) return node.data === "" ? NodeFilter.FILTER_SKIP : NodeFilter.FILTER_ACCEPT;
      if (node instanceof Element && (node.matches(SKIP) || unseen(node))) return NodeFilter.FILTER_REJECT;
      return NodeFilter.FILTER_SKIP;
    },
  });
  const nodes: Text[] = [];
  const starts: number[] = [];
  let text = "";
  let lastBlock: Element | null = null;
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    if (!(node instanceof Text)) continue;
    const block = node.parentElement?.closest(BLOCK) ?? null;
    if (nodes.length > 0 && block !== lastBlock) text += "\n";
    lastBlock = block;
    starts.push(text.length);
    nodes.push(node);
    text += node.data;
  }
  const haystack = lower(text);
  const needle = lower(query);
  /** The text node holding character `at` of the whole text, and the offset in it. */
  const locate = (at: number): [Text, number] | null => {
    let low = 0;
    let high = starts.length - 1;
    while (low < high) {
      const middle = Math.ceil((low + high) / 2);
      if ((starts[middle] ?? 0) <= at) low = middle;
      else high = middle - 1;
    }
    const node = nodes[low];
    return node ? [node, at - (starts[low] ?? 0)] : null;
  };
  const ranges: Range[] = [];
  for (let at = haystack.indexOf(needle); at >= 0; at = haystack.indexOf(needle, at + needle.length)) {
    if (ranges.length === MAX_RESULTS) return { ranges, more: true };
    const start = locate(at);
    const end = locate(at + needle.length - 1);
    if (!start || !end) break;
    const range = document.createRange();
    range.setStart(start[0], start[1]);
    range.setEnd(end[0], end[1] + 1);
    ranges.push(range);
  }
  return { ranges, more: false };
}

function reducedMotion(): boolean {
  return globalThis.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
}

const nextFrame = () => new Promise<number>((resolve) => requestAnimationFrame(resolve));

/** The highlight registry, where the webview has one. */
function registry(): HighlightRegistry | null {
  if (typeof CSS === "undefined" || !("highlights" in CSS) || typeof Highlight === "undefined") return null;
  return CSS.highlights;
}

/**
 * The marks are changed in place, never swapped or dropped whole: WebKit repaints the ranges a
 * highlight gains or loses, where a registry entry replaced or deleted left the old marks on screen.
 */
function marks(highlights: HighlightRegistry, name: string, priority: number): Highlight {
  let highlight = highlights.get(name);
  if (!highlight) {
    highlight = new Highlight();
    highlight.priority = priority;
    highlights.set(name, highlight);
  }
  return highlight;
}

/**
 * Marks `ranges` in `viewport`'s thread, `active` over them. WebKit paints a mark only on text
 * that can be selected, and puts one on unselectable text (a Shell box's output, a row) onto the
 * next selectable text instead, so the thread's text is selectable while it holds marks. Text in a
 * button never is: the button holding the match gone to gets a ring.
 */
function paint(viewport: HTMLElement, ranges: readonly Range[], active: Range | null): void {
  const highlights = registry();
  if (!highlights) return;
  viewport.setAttribute(FINDING, "");
  const all = marks(highlights, MATCHES, 0);
  all.clear();
  for (const range of ranges) all.add(range);
  // Over the others where they meet.
  const current = marks(highlights, ACTIVE_MATCH, 1);
  current.clear();
  if (active) current.add(active);
  const ringed = active?.startContainer.parentElement?.closest("button") ?? null;
  for (const button of viewport.querySelectorAll(`[${ACTIVE_BUTTON}]`)) {
    if (button !== ringed) button.removeAttribute(ACTIVE_BUTTON);
  }
  if (ringed && viewport.contains(ringed)) ringed.setAttribute(ACTIVE_BUTTON, "");
}

/** Takes the marks off, then the thread's text back to what it was. */
function unpaint(viewport: HTMLElement | null): void {
  const highlights = registry();
  for (const name of [MATCHES, ACTIVE_MATCH]) {
    highlights?.get(name)?.clear();
    highlights?.delete(name);
  }
  if (!viewport) return;
  for (const button of viewport.querySelectorAll(`[${ACTIVE_BUTTON}]`)) button.removeAttribute(ACTIVE_BUTTON);
  viewport.removeAttribute(FINDING);
}

/**
 * The user message's "Show more" that hides `range` in its clipped tail, if one does: its text
 * counts, and going to it opens the message.
 */
function clippedBy(range: Range): HTMLButtonElement | null {
  const start = range.startContainer.parentElement;
  const bubble = start?.closest(".aui-user-message-content");
  const clip = bubble?.firstElementChild;
  const more = bubble?.querySelector<HTMLButtonElement>(':scope > button[aria-expanded="false"]');
  if (!clip || !more) return null;
  return range.getBoundingClientRect().bottom > clip.getBoundingClientRect().bottom + 1 ? more : null;
}

/** Whether key press `event` is ⌘ and `code` (Ctrl off macOS), with Shift as `shift` says. */
function command(event: KeyboardEvent, mac: boolean, code: string): boolean {
  const held = mac ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey;
  return held && !event.altKey && event.code === code && !event.isComposing;
}

/** Where a key press belongs to something else's own find, or to no thread at all. */
const ELSEWHERE = '[data-slot="terminal-pane"], [data-pane="browser"], [data-slot="side-panel-clip"], [role="tabpanel"], [role="dialog"]';

/**
 * The find bar of one thread, mounted in its root beside the viewport. Only the conversation's
 * own thread finds: a side chat's, embedded in a panel, leaves ⌘F to what holds it.
 */
export const ThreadFind: FC<ThreadOverlayProps> = ({ viewport, scroller }) => {
  const mac = useApp((s) => s.info?.platform === "macos");
  const history = useContext(ThreadHistoryContext);
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [result, setResult] = useState<FindResult>(NONE);
  const [active, setActive] = useState(-1);
  /** Bumped by each ⌘F: the field takes the focus with its text selected. */
  const [focusAsk, setFocusAsk] = useState(0);
  const input = useRef<HTMLInputElement>(null);
  const bar = useRef<HTMLDivElement>(null);
  /** What had the keyboard before the bar took it, given back on closing. */
  const before = useRef<Element | null>(null);
  /** Stepping back past the oldest match unfolded the earlier turns: the match before this one is next. */
  const backFrom = useRef<Range | null>(null);
  const reveals = useRef(0);

  // What the listeners and reruns read: the newest, not what was there when they were made.
  const state = useRef({ open, query, result, active, history });
  const rerun = useRef<() => void>(() => {});
  useLayoutEffect(() => {
    state.current = { open, query, result, active, history };
  });

  /** Whether this thread is the one ⌘F finds in now: shown, in front, not a side chat's. */
  const finds = useCallback(
    (target: EventTarget | null): boolean => {
      if (!viewport?.isConnected || viewport.clientHeight === 0) return false;
      if (viewport.closest("[data-embedded-view], [inert]")) return false;
      const root = viewport.closest(".aui-thread-root");
      const element = target instanceof Element ? target : null;
      const thread = element?.closest(".aui-thread-root");
      if (thread) return thread === root;
      return !element?.closest(ELSEWHERE);
    },
    [viewport],
  );

  /** Brings `range` into view, centred between the bar and the composer, unless it shows already. */
  const reveal = useCallback(
    async (range: Range) => {
      if (!viewport) return;
      const seq = ++reveals.current;
      const live = () => seq === reveals.current && viewport.isConnected && range.startContainer.isConnected;
      const more = clippedBy(range);
      if (more) {
        more.click();
        for (let frames = 0; frames < 30 && live() && more.isConnected && more.getAttribute("aria-expanded") !== "true"; frames++) await nextFrame();
        await nextFrame();
        if (!live()) return;
      }
      // The user's own move: following the bottom stops, as their scroll away from it would.
      scroller.release();
      const box = viewport.getBoundingClientRect();
      const footer = viewport.parentElement?.querySelector<HTMLElement>("[data-thread-scroll-footer]")?.offsetHeight ?? 0;
      const barBottom = bar.current ? bar.current.getBoundingClientRect().bottom : box.top;
      const top = Math.max(box.top, barBottom) + CLEAR_PX;
      const bottom = Math.max(top, box.bottom - footer - CLEAR_PX);
      // In a box that scrolls on its own (a Shell box's output, an opened group's list): each, the
      // innermost first, brings the match to its middle, before the thread does.
      for (let own = range.startContainer.parentElement; own && own !== viewport && viewport.contains(own); own = own.parentElement) {
        if (own.scrollHeight <= own.clientHeight + 1 || !/auto|scroll/.test(getComputedStyle(own).overflowY)) continue;
        const sides = own.getBoundingClientRect();
        const at = range.getBoundingClientRect();
        if (at.top < sides.top || at.bottom > sides.bottom) own.scrollTop += at.top - sides.top - (sides.height - at.height) / 2;
      }
      const rect = range.getBoundingClientRect();
      // In a code block that scrolls sideways: it scrolls to the match.
      const pre = range.startContainer.parentElement?.closest("pre");
      if (pre && pre.scrollWidth > pre.clientWidth) {
        const sides = pre.getBoundingClientRect();
        if (rect.left < sides.left || rect.right > sides.right) pre.scrollLeft += rect.left - sides.left - sides.width / 2;
      }
      if (rect.top >= top && rect.bottom <= bottom) return;
      const max = Math.max(0, viewport.scrollHeight - viewport.clientHeight);
      const target = viewport.scrollTop + rect.top + rect.height / 2 - (top + bottom) / 2;
      // A hidden window holds a smooth scroll still: there it jumps.
      viewport.scrollTo({ top: Math.round(Math.max(0, Math.min(max, target))), behavior: reducedMotion() || document.hidden ? "instant" : "smooth" });
    },
    [viewport, scroller],
  );

  const goTo = useCallback(
    (index: number, ranges: readonly Range[]) => {
      setActive(index);
      const range = ranges[index];
      if (range) void reveal(range);
    },
    [reveal],
  );

  /** The match after (`1`) or before (`-1`) the one gone to, round from the last to the first. */
  const step = useCallback(
    (by: 1 | -1) => {
      const { result: found, active: at, history: folds } = state.current;
      const count = found.ranges.length;
      if (count === 0) return;
      const oldest = found.ranges[0];
      if (by < 0 && at <= 0 && folds && folds.hidden > 0 && oldest) {
        // The earlier turns are searched too: they open, and the match before this one is next.
        backFrom.current = oldest.cloneRange();
        folds.unfold();
        return;
      }
      goTo((at + by + count) % count, found.ranges);
    },
    [goTo],
  );

  /** Searches for `text`; `fresh` (the text changed) goes to the first match from the view's top. */
  const run = useCallback(
    (text: string, fresh: boolean) => {
      const { result: last, active: at } = state.current;
      const column = viewport?.querySelector('[data-slot="aui_thread-column"]');
      const found = column ? findIn(column, text) : NONE;
      setResult(found);
      const { ranges } = found;
      if (ranges.length === 0) {
        setActive(-1);
        return;
      }
      const back = backFrom.current;
      if (back) {
        backFrom.current = null;
        const earlier = back.startContainer.isConnected
          ? ranges.findLastIndex((range) => range.compareBoundaryPoints(Range.START_TO_START, back) < 0)
          : -1;
        goTo(earlier >= 0 ? earlier : ranges.length - 1, ranges);
        return;
      }
      if (fresh || at < 0) {
        const line = viewport ? viewport.getBoundingClientRect().top : 0;
        const first = ranges.findIndex((range) => range.getBoundingClientRect().bottom > line);
        goTo(first >= 0 ? first : 0, ranges);
        return;
      }
      // The thread changed under the bar: the match gone to stays, where it still is.
      const kept = last.ranges[at];
      const same = kept ? ranges.findIndex((range) => range.startContainer === kept.startContainer && range.startOffset === kept.startOffset) : -1;
      setActive(same >= 0 ? same : Math.min(at, ranges.length - 1));
    },
    [viewport, goTo],
  );

  useLayoutEffect(() => {
    rerun.current = () => run(state.current.query, false);
  });

  const close = useCallback(() => {
    reveals.current++;
    backFrom.current = null;
    setOpen(false);
    setResult(NONE);
    setActive(-1);
    unpaint(viewport);
    const back = before.current;
    before.current = null;
    // Where it is, without scrolling the thread there: the reader keeps their place.
    if (back instanceof HTMLElement && back.isConnected && back !== document.body) back.focus({ preventScroll: true });
  }, [viewport]);

  /** ⌘F: opens the bar (or takes the focus back to it), with a single line selected in the thread as its text. */
  const openFind = useCallback(() => {
    const selection = getSelection();
    let text = state.current.query;
    if (selection && !selection.isCollapsed && viewport?.contains(selection.anchorNode) && viewport.contains(selection.focusNode)) {
      const picked = selection.toString().trim();
      text = /[\r\n]/.test(picked) ? "" : picked;
    }
    const fresh = !state.current.open || text !== state.current.query;
    setQuery(text);
    if (fresh) run(text, true);
    if (!state.current.open) {
      const focused = document.activeElement;
      before.current = focused && !bar.current?.contains(focused) ? focused : null;
      setOpen(true);
      // The turns folded behind "N earlier messages" are searched too.
      if (state.current.history && state.current.history.hidden > 0) state.current.history.unfold();
    }
    setFocusAsk((ask) => ask + 1);
  }, [viewport, run]);

  // ⌘F, and ⌘G / ⇧⌘G while the bar is open (Ctrl off macOS).
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented) return;
      if (command(event, mac, "KeyF") && !event.shiftKey) {
        if (!finds(event.target)) return;
        event.preventDefault();
        openFind();
      } else if (command(event, mac, "KeyG") && state.current.open) {
        if (!finds(event.target)) return;
        event.preventDefault();
        step(event.shiftKey ? -1 : 1);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [mac, finds, openFind, step]);

  useLayoutEffect(() => {
    if (focusAsk === 0) return;
    input.current?.focus();
    input.current?.select();
  }, [focusAsk]);

  // While the bar is open the thread's changes are searched again, a moment after they come.
  useEffect(() => {
    if (!open || !viewport) return;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const observer = new MutationObserver(() => {
      timer ??= setTimeout(() => {
        timer = null;
        rerun.current();
      }, RERUN_MS);
    });
    observer.observe(viewport, { childList: true, subtree: true, characterData: true });
    return () => {
      observer.disconnect();
      if (timer) clearTimeout(timer);
    };
  }, [open, viewport]);

  useLayoutEffect(() => {
    if (open && viewport) paint(viewport, result.ranges, result.ranges[active] ?? null);
  }, [open, viewport, result, active]);

  // Gone with the thread: so are its marks.
  useEffect(() => () => unpaint(viewport), [viewport]);

  if (!open) return null;

  const count = result.ranges.length;
  // Esc closes from the field or a button, and goes no further (the composer's Esc Esc stops a reply).
  const onEscape = (event: ReactKeyboardEvent) => {
    if (event.key !== "Escape" || event.nativeEvent.isComposing) return;
    event.preventDefault();
    event.stopPropagation();
    close();
  };

  return (
    <div
      ref={bar}
      role="search"
      aria-label="Find in chat"
      data-slot="thread-find"
      className="bg-popover text-popover-foreground shadow-browser-find rounded-menu h-browser-find-row w-thread-find max-w-thread-find-max absolute top-2 right-4 z-30 flex min-w-0 items-center gap-2 ps-4 pe-2"
    >
      <Search aria-hidden className="text-muted-foreground size-icon-md shrink-0" />
      <input
        ref={input}
        value={query}
        placeholder="Search chat…"
        aria-label="Find in chat"
        spellCheck={false}
        autoCapitalize="off"
        autoCorrect="off"
        onChange={(event) => {
          // A new text is searched at once.
          setQuery(event.target.value);
          run(event.target.value, true);
        }}
        onKeyDown={(event) => {
          if (event.key === "Enter" && !event.nativeEvent.isComposing) {
            event.preventDefault();
            step(event.shiftKey ? -1 : 1);
          } else onEscape(event);
        }}
        className="text-browser placeholder:text-muted-foreground h-6 min-w-0 flex-1 bg-transparent outline-none"
      />
      {query !== "" && (
        <span
          data-slot="thread-find-count"
          aria-live="polite"
          className="text-muted-foreground shrink-0 text-xs whitespace-nowrap tabular-nums select-none"
        >
          {findLabel(active, count, result.more)}
        </span>
      )}
      <TooltipIconButton tooltip="Previous result" shortcut="⇧↵" size="icon-xs" data-find="previous" onKeyDown={onEscape} disabled={count === 0} onClick={() => step(-1)}>
        <ChevronLeft />
      </TooltipIconButton>
      <TooltipIconButton tooltip="Next result" shortcut="↵" size="icon-xs" data-find="next" onKeyDown={onEscape} disabled={count === 0} onClick={() => step(1)}>
        <ChevronRight />
      </TooltipIconButton>
      <span aria-hidden className="bg-border h-4 w-px shrink-0" />
      <TooltipIconButton tooltip="Close find" size="icon-xs" data-find="close" onKeyDown={onEscape} onClick={close}>
        <X />
      </TooltipIconButton>
    </div>
  );
};
