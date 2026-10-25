import { useCallback, useContext, useEffect, useLayoutEffect, useRef, useState, type FC, type KeyboardEvent as ReactKeyboardEvent } from "react";

import type { BlockMeta } from "@/app/conversation/RequestBlock";
import { ThreadHistoryContext, type ThreadMessage, type ThreadOverlayProps, useThreadKey, keptChanges, rowKeyOf, useThreadListIds, useThreadListSource, useAui, placeOf } from "@/components/conversation";
import { TooltipIconButton } from "@/components/controls";
import { ChevronLeft, ChevronRight, Search, X } from "@/components/icons";
import type { MessageMatch } from "@/ipc/generated";
import { startSearching } from "@/lib/findSearching";
import { showHighlight } from "@/lib/highlightRules";
import { useBoard } from "@/state/board";
import { useApp } from "@/state/store";
import { PLAIN_FIELD } from "@/lib/plainTyping";

/*
 * Find in the chat (⌘F with the chat in front): a bar at the top right of the thread finds text
 * in what the thread shows, case aside: the user's messages and the answers, work rows' labels,
 * the lines between steps. Every match is washed yellow and the one gone to orange, through the
 * CSS Custom Highlight API, so the thread's DOM is never touched. Enter, ⌘G and › go to the next
 * one, ⇧Enter, ⇧⌘G and ‹ the one before, Esc closes.
 *
 * It searches what the thread shows: a turn's closed work and the closed work groups in it keep
 * their rows, hidden, while the bar has a text (`useFindSearching`): going to a match there opens
 * the work or the group, typing never does. The thread mounts only the messages near the view
 * (see `thread.tsx`): in the others it finds what they showed when last mounted, or else their
 * words as the app holds them (`wordsOf`: what they say, not every label they draw), so their
 * matches count in their places; going to one mounts its message and finds it there again. Pages still in the database are searched by the
 * engine (`searchOlder`), which counts their matches ahead of the thread's own; going to one loads
 * the pages down to it. Diffs draw in their own shadow tree and are not searched.
 */

/** Counting stops here: "250+ results". */
export const MAX_RESULTS = 250;
/** The highlights the matches are drawn with (globals.css). */
export const MATCHES = "thread-find" as const;
export const ACTIVE_MATCH = "thread-find-active" as const;
/** On the thread while it holds marks: its text is selectable (globals.css). */
export const FINDING = "data-thread-finding";
/** On a button holding the match gone to, whose text WebKit never marks. */
export const ACTIVE_BUTTON = "data-thread-find-active";
/** How soon after the thread changes the search runs again, while the bar is open. */
const RERUN_MS = 150;
/** How long typing rests before the pages not loaded are searched. */
const OLDER_MS = 150;
/** Pages loaded on the way to a match in one before the bar gives up. */
const OLDER_PAGES = 50;
/** A match in a page just loaded is gone to once it holds its place this many frames. */
const STILL_FRAMES = 3;
/** ...and at the latest this many frames after its page loaded. */
const LAST_FRAME = 300;
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

// While a bar has a text, a turn's closed work keeps its rows, hidden, for it (lib/findSearching.ts).
export { useFindSearching } from "@/lib/findSearching";

/** The closed work fold holding `range`, if one does. */
function closedFold(range: Range): Element | null {
  return range.startContainer.parentElement?.closest('[data-slot="request-fold"][data-fold="closed"]') ?? null;
}

/** The closed work group holding `range` in the rows it keeps for Find, if one does. */
function closedGroup(range: Range): Element | null {
  return range.startContainer.parentElement?.closest('[data-slot="work-group-kept"]')?.closest('[data-slot="work-group"]') ?? null;
}

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

/** A match in a message the thread hasn't mounted: its message, and which of its matches it is. */
export type Away = { row: string; nth: number };
/** A match: in the thread's DOM, or in a message not mounted. */
export type Hit = Range | Away;

export function isRange(hit: Hit | undefined): hit is Range {
  return typeof Range !== "undefined" && hit instanceof Range;
}

export type FindResult = { ranges: Hit[]; more: boolean };

const NONE: FindResult = { ranges: [], more: false };

/** The engine's matches of a text in the pages not loaded, as it last told them. */
type Older = { query: string; matches: readonly MessageMatch[]; more: boolean };

/**
 * What the bar found: the matches in pages not loaded (`before` of them, oldest first) and then
 * the thread's own `ranges`. A match's index counts from the first of the older ones.
 */
type Found = FindResult & { before: number; older: readonly MessageMatch[] };

const NOTHING: Found = { ...NONE, before: 0, older: [] };

/**
 * `found`'s matches in the thread with those of `older` in pages older than `oldest` (the
 * oldest message loaded) ahead of them, the newest of those kept when they all pass `MAX_RESULTS`.
 */
function withOlder(found: FindResult, older: Older | null, query: string, oldest: number | null): Found {
  if (!older || older.query !== query || query === "") return { ...found, before: 0, older: [] };
  const unloaded = older.matches.filter((match) => oldest === null || match.seq < oldest);
  const all = unloaded.reduce((sum, match) => sum + match.count, 0);
  const before = Math.max(0, Math.min(all, MAX_RESULTS - found.ranges.length));
  return { ranges: found.ranges, more: found.more || older.more || before < all, before, older: unloaded };
}

/** The message in a page not loaded holding match `index` of `found` (one of its first `before`). */
function olderAt(found: Found, index: number): MessageMatch | null {
  // Counted from the newest: when they pass the limit, the oldest are the ones left out.
  let left = found.before - 1 - index;
  for (const match of found.older.toReversed()) {
    if (left < match.count) return match;
    left -= match.count;
  }
  return null;
}

/** The text `root` shows, as Find reads it, with the text nodes it is made of and where each starts. */
function textIn(root: Element): { text: string; nodes: Text[]; starts: number[] } {
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
  return { text, nodes, starts };
}

/** Markdown's marks, which a message's text drawn shows without: fences, heads, quotes, list marks, links' targets, emphasis. */
function unmarked(text: string): string {
  return text
    .replace(/^\s*```.*$/gm, "")
    .replace(/^\s{0,3}(?:#{1,6}\s+|>\s?|[-*+]\s+|\d+[.)]\s+)/gm, "")
    .replace(/!?\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/\*\*|__|~~|`/g, "");
}

/** What each message says, lower case, made once per message. */
const wordsBy = new WeakMap<ThreadMessage, string>();

/** A work step's fields its row draws as words (a tool's name and what it ran on, a line's text). */
const STEP_WORDS = ["name", "detail", "text", "title", "reason", "what", "why", "command"] as const;

/** What a turn's work says: its steps' words and the messages steered into it. */
function workWords(meta: BlockMeta | undefined): string[] {
  if (!meta) return [];
  const words: string[] = [];
  for (const step of meta.orchestratorSteps) {
    const kind = step.kind as Partial<Record<(typeof STEP_WORDS)[number], unknown>>;
    for (const key of STEP_WORDS) if (typeof kind[key] === "string") words.push(kind[key]);
  }
  for (const steer of meta.steers) words.push(steer.text);
  return words;
}

/**
 * What `message` says, lower case, as Find reads a message the thread hasn't mounted (and never
 * did since it last changed): its text, the answer's without Markdown's marks, and its work's words
 * (shown or closed, as Find searches closed work too).
 */
export function wordsOf(message: ThreadMessage): string {
  let words = wordsBy.get(message);
  if (words === undefined) {
    words = lower(
      [
        ...message.content.map((part) => (part.type !== "text" ? "" : message.role === "assistant" ? unmarked(part.text) : part.text)),
        ...workWords(message.metadata.custom["block"] as BlockMeta | undefined),
      ]
        .filter(Boolean)
        .join("\n"),
    );
    wordsBy.set(message, words);
  }
  return words;
}

/**
 * The matches of `query` in the text `root` shows, top to bottom, the newest `MAX_RESULTS` of them,
 * and that text, lower case, as `shownText` reads it.
 */
export function findIn(root: Element, query: string): { ranges: Range[]; more: boolean; text: string } {
  if (query === "") return { ranges: [], more: false, text: "" };
  const { text, nodes, starts } = textIn(root);
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
  // Like the engine, retain the newest matches, still ordered from oldest to newest.
  const positions: number[] = [];
  let count = 0;
  for (let at = haystack.indexOf(needle); at >= 0; at = haystack.indexOf(needle, at + needle.length)) {
    positions[count % MAX_RESULTS] = at;
    count++;
  }
  const ranges: Range[] = [];
  for (let index = Math.max(0, count - MAX_RESULTS); index < count; index++) {
    const at = positions[index % MAX_RESULTS]!;
    const start = locate(at);
    const end = locate(at + needle.length - 1);
    if (!start || !end) break;
    const range = document.createRange();
    range.setStart(start[0], start[1]);
    range.setEnd(end[0], end[1] + 1);
    ranges.push(range);
  }
  return { ranges, more: count > MAX_RESULTS, text: haystack };
}

/**
 * The matches of `query` in the thread, oldest first, the newest `MAX_RESULTS` of them: in the
 * messages `group` mounts, in its DOM; in the others, in their text as `texts` gives it, lower
 * case. `ids`: the thread's messages, in order. What each mounted message shows goes to `read`,
 * kept for when it isn't mounted any more.
 */
export function findInThread(group: Element, ids: readonly string[], texts: (id: string) => string, query: string, read?: Map<string, string>): FindResult {
  if (query === "") return NONE;
  const mounted = new Map<string, Element>();
  for (const row of group.querySelectorAll<HTMLElement>(":scope > [data-thread-row]")) mounted.set(row.dataset["threadRow"] ?? "", row);
  const needle = lower(query);
  // The newest first, until the limit: older messages aren't searched once it is passed.
  const newest: Hit[][] = [];
  let count = 0;
  let more = false;
  for (let index = ids.length - 1; index >= 0 && !more; index--) {
    const id = ids[index] ?? "";
    const row = mounted.get(id);
    let hits: Hit[] = [];
    if (row) {
      const found = findIn(row, query);
      hits = found.ranges;
      more = found.more;
      read?.set(id, found.text);
    } else {
      const text = texts(id);
      for (let at = text.indexOf(needle), nth = 0; at >= 0; at = text.indexOf(needle, at + needle.length), nth++) hits.push({ row: id, nth });
    }
    count += hits.length;
    if (count > MAX_RESULTS) more = true;
    newest.push(hits);
  }
  const all = newest.toReversed().flat();
  return { ranges: all.slice(Math.max(0, all.length - MAX_RESULTS)), more };
}

/** The message of the thread holding `hit`. */
function rowOf(hit: Hit): string | null {
  if (!isRange(hit)) return hit.row;
  return hit.startContainer.parentElement?.closest<HTMLElement>("[data-thread-row]")?.dataset["threadRow"] ?? null;
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

/** Paint each text run separately: inline code has its own padding and font metrics.
 * Button text is found and ringed, but WebKit can paint its highlight on neighbouring text. */
function paintRanges(range: Range): Range[] {
  const root = range.commonAncestorContainer;
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  const pieces: Range[] = [];
  for (let node = root instanceof Text ? root : walker.nextNode(); node; node = walker.nextNode()) {
    if (!(node instanceof Text) || !range.intersectsNode(node) || node.parentElement?.closest("button")) continue;
    const start = node === range.startContainer ? range.startOffset : 0;
    const end = node === range.endContainer ? range.endOffset : node.length;
    if (start === end) continue;
    const piece = document.createRange();
    piece.setStart(node, start);
    piece.setEnd(node, end);
    pieces.push(piece);
  }
  return pieces;
}

/**
 * Marks `ranges` in `viewport`'s thread, `active` over them. WebKit paints a mark only on text
 * that can be selected, and puts one on unselectable text (a Shell box's output, a row) onto the
 * next selectable text instead, so the thread's text is selectable while it holds marks. Text in a
 * button never is: the button holding the match gone to gets a ring.
 */
function paint(viewport: HTMLElement, hits: readonly Hit[], hit: Hit | null): void {
  const ranges = hits.filter(isRange);
  const active = isRange(hit ?? undefined) ? (hit as Range) : null;
  const highlights = registry();
  if (!highlights) return;
  viewport.setAttribute(FINDING, "");
  showHighlight(MATCHES, true);
  showHighlight(ACTIVE_MATCH, true);
  const all = marks(highlights, MATCHES, 0);
  all.clear();
  for (const range of ranges) for (const piece of paintRanges(range)) all.add(piece);
  // Over the others where they meet.
  const current = marks(highlights, ACTIVE_MATCH, 1);
  current.clear();
  if (active) for (const piece of paintRanges(active)) current.add(piece);
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
    showHighlight(name, false);
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
  const aui = useAui();
  const list = useThreadListSource();
  const threadKey = useThreadKey();
  const ids = useThreadListIds();
  const idsRef = useRef(ids);
  /** What each message showed when last mounted, and what it was drawn from. */
  const texts = useRef(new Map<string, Indexed>());
  /** The thread's whole branch now (assistant-ui holds only some of it, see `thread-list.ts`). */
  const messagesNow = useCallback(() => list?.get().messages ?? aui.thread().getState().messages, [list, aui]);
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [result, setResult] = useState<Found>(NOTHING);
  const [active, setActive] = useState(-1);
  /** Bumped by each ⌘F: the field takes the focus with its text selected. */
  const [focusAsk, setFocusAsk] = useState(0);
  const input = useRef<HTMLInputElement>(null);
  const bar = useRef<HTMLDivElement>(null);
  /** What had the keyboard before the bar took it, given back on closing. */
  const before = useRef<Element | null>(null);
  const reveals = useRef(0);
  const expanded = useRef(new Set<Element>());
  const toggling = useRef(false);
  const closingAnchor = useRef<{ element: HTMLElement; top: number } | null>(null);
  /** The engine's matches in the pages not loaded, for the text it was asked last. */
  const older = useRef<Older | null>(null);
  /** A step to match `index` loads pages: until the thread counts `all` matches again, the bar keeps what it shows. */
  const loading = useRef<{ index: number; all: number } | null>(null);

  // What the listeners and reruns read: the newest, not what was there when they were made.
  const state = useRef({ open, query, result, active, history });
  const rerun = useRef<() => void>(() => {});
  /** Keeps what messages showed, by id, with what they were drawn from (see `freshIndex`). */
  const keepTexts = useRef<(read: ReadonlyMap<string, string>) => void>(() => {});
  /** A message's text as Find reads it while not mounted: what it showed last, while that holds, or else its words. */
  const textOf = useRef<(id: string) => string>(() => "");
  /** The search again, the match gone to kept: what a step reads after it opened or loaded something. */
  const research = useRef<() => Found>(() => NOTHING);
  useLayoutEffect(() => {
    state.current = { open, query, result, active, history };
    idsRef.current = ids;
  });
  /** Match `index` is the one gone to: what `state` reads at once, as the result it belongs to is. */
  const choose = useCallback((index: number) => {
    state.current.active = index;
    setActive(index);
  }, []);

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

  /**
   * Brings `range` (match `index`) into view, centred between the bar and the composer, unless it
   * shows already. In a turn's closed work, or a closed group in it, it opens them first when
   * `opens` (a step, never typing), else stays.
   */
  const reveal = useCallback(
    async (match: Range, index: number, opens: boolean) => {
      if (!viewport) return;
      let range = match;
      const seq = ++reveals.current;
      const live = () => seq === reveals.current && viewport.isConnected && range.startContainer.isConnected;
      const fold = closedFold(range);
      if (fold) {
        const header = fold.closest('[data-slot="request-body"]')?.querySelector<HTMLButtonElement>('button[data-slot="request-work-header"][aria-expanded="false"]');
        if (!opens || !header) return;
        // As the user's own click: the turn's work stays open after the bar closes.
        header.click();
        const opened = () => fold.getAttribute("data-fold") === "open" && !fold.hasAttribute("data-moving");
        for (let frames = 0; frames < 60 && live() && fold.isConnected && !opened(); frames++) await nextFrame();
        // Its last resize is the thread's to keep its place by first, or that undoes the scroll.
        await nextFrame();
        if (!live()) return;
      }
      const group = closedGroup(range);
      if (group) {
        const trigger = group.querySelector<HTMLButtonElement>(':scope > [data-slot="collapsible-trigger"][aria-expanded="false"]');
        if (!opens || !trigger) return;
        // As the user's own click. Opened, the group draws its rows anew, so the match is found
        // again: the same one, at the same index.
        trigger.click();
        const going = () => seq === reveals.current && viewport.isConnected && group.isConnected;
        const content = () => group.querySelector(':scope > [data-slot="collapsible-content"][data-state="open"]');
        const opened = () => !group.querySelector(':scope > [data-slot="work-group-kept"]') && content()?.getAnimations().every((each) => each.playState !== "running");
        for (let frames = 0; frames < 60 && going() && !opened(); frames++) await nextFrame();
        await nextFrame();
        if (!going()) return;
        const found = research.current();
        const again = found.ranges[index - found.before];
        if (!isRange(again) || closedGroup(again)) return;
        choose(index);
        range = again;
      }
      const more = clippedBy(range);
      if (more) {
        const bubble = more.closest(".aui-user-message-content");
        if (bubble) expanded.current.add(bubble);
        toggling.current = true;
        more.click();
        toggling.current = false;
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
      /** Where the view's top goes for the match to sit midway, within what the view can reach. */
      const aim = () => {
        const max = Math.max(0, viewport.scrollHeight - viewport.clientHeight);
        const at = range.getBoundingClientRect();
        return Math.round(Math.max(0, Math.min(max, viewport.scrollTop + at.top + at.height / 2 - (top + bottom) / 2)));
      };
      // A hidden window holds a smooth scroll still: there it jumps.
      viewport.scrollTo({ top: aim(), behavior: reducedMotion() || document.hidden ? "instant" : "smooth" });
      // The messages mounting on the way at their own heights move the match, or stop the glide:
      // once the view rests, it lands where it should all the same.
      for (let tries = 0; tries < 3; tries++) {
        let last = viewport.scrollTop;
        for (let still = 0, frames = 0; still < STILL_FRAMES && frames < 120 && live(); frames++) {
          await nextFrame();
          still = viewport.scrollTop === last ? still + 1 : 0;
          last = viewport.scrollTop;
        }
        if (!live() || Math.abs(aim() - viewport.scrollTop) <= 1) return;
        viewport.scrollTo({ top: aim(), behavior: "instant" });
        scroller.scrolled();
      }
    },
    [viewport, scroller, choose],
  );

  /**
   * Goes to `hit`, a match in a message the thread hasn't mounted: the message is mounted, and
   * once it holds its place its match is found again in what it shows now (the same one of its
   * matches, or its last if it has fewer now) and gone to.
   */
  const bring = useCallback(
    async (hit: Away, opens: boolean) => {
      if (!viewport) return;
      const seq = ++reveals.current;
      const live = () => seq === reveals.current && viewport.isConnected;
      scroller.release();
      scroller.mount("find", hit.row);
      const selector = `[data-slot="aui_message-group"] > [data-thread-row="${CSS.escape(hit.row)}"]`;
      let last: number | null = null;
      let still = 0;
      for (let frames = 0; frames < 120 && live(); frames++) {
        await nextFrame();
        const row = viewport.querySelector(selector);
        if (!row) continue;
        const top = row.getBoundingClientRect().top;
        still = top === last ? still + 1 : 0;
        last = top;
        if (still < STILL_FRAMES - 1) continue;
        const found = research.current();
        const mine = found.ranges.filter((each): each is Range => isRange(each) && rowOf(each) === hit.row);
        const range = mine[Math.min(hit.nth, mine.length - 1)];
        if (!range) return;
        const at = found.before + found.ranges.indexOf(range);
        choose(at);
        void reveal(range, at, opens);
        return;
      }
    },
    [viewport, scroller, reveal, choose],
  );

  /**
   * Goes to match `index` of `found`, one in a page not loaded: the pages down to its message
   * load, and once the thread shows it, it is found there and gone to. It is found counting back
   * from the thread's first match before the load (as many matches back as it was from the
   * newest one not loaded), so a count the engine got wrong further back never moves it.
   */
  const loadTo = useCallback(
    async (index: number, found: Found) => {
      const target = olderAt(found, index);
      if (!viewport || !target) return;
      const seq = ++reveals.current;
      const live = () => seq === reveals.current && viewport.isConnected;
      const all = found.before + found.ranges.length;
      const back = found.before - 1 - index;
      const anchor = found.ranges.find(isRange)?.cloneRange() ?? null;
      const mine = { index, all };
      loading.current = mine;
      const going = () => {
        const on = live();
        if (!on && loading.current === mine) loading.current = null;
        return on;
      };
      for (let page = 0; page < OLDER_PAGES; page++) {
        const pages = state.current.history;
        if (!pages?.hasMore || (pages.oldest !== null && pages.oldest <= target.seq)) break;
        const loaded = await pages.loadOlder().then(
          () => true,
          () => false,
        );
        if (!loaded) {
          if (loading.current === mine) loading.current = null;
          return;
        }
        // The thread holds the page once its history says so.
        for (let frames = 0; frames < 600 && live() && state.current.history?.oldest === pages.oldest; frames++) await nextFrame();
        if (!going()) return;
      }
      // Shown once the thread counts as many matches as before, its own now (a long message's
      // tail loads after its start), and holding its place: the thread keeps the reader's while
      // the pages above it settle. A count the engine got wrong waits no longer than `LAST_FRAME`.
      let last: number | null = null;
      let still = 0;
      for (let frames = 0; frames <= LAST_FRAME && going(); frames++) {
        await nextFrame();
        const now = research.current();
        if (now.before + now.ranges.length < all && frames < LAST_FRAME) continue;
        const shown = now.ranges.filter(isRange);
        const loaded = anchor?.startContainer.isConnected
          ? shown.filter((range) => range.compareBoundaryPoints(Range.START_TO_START, anchor) < 0)
          : shown;
        // Counted further back than the thread shows: the oldest it shows, once the wait is up.
        const range = loaded[loaded.length - 1 - back] ?? (frames === LAST_FRAME ? loaded[0] : undefined);
        if (!range) continue;
        const top = range.getBoundingClientRect().top;
        still = top === last ? still + 1 : 0;
        last = top;
        if (still < STILL_FRAMES && frames < LAST_FRAME) continue;
        const at = now.before + now.ranges.indexOf(range);
        loading.current = null;
        state.current.result = now;
        setResult(now);
        choose(at);
        void reveal(range, at, true);
        return;
      }
      if (loading.current === mine) loading.current = null;
    },
    [viewport, reveal, choose],
  );

  /** Match `index` of `found` is the one gone to; `opens` (a step) goes there: opening, loading what it must. */
  const goTo = useCallback(
    (index: number, found: Found, opens: boolean) => {
      choose(index);
      if (index < found.before) {
        if (opens) void loadTo(index, found);
        return;
      }
      const hit = found.ranges[index - found.before];
      if (isRange(hit)) void reveal(hit, index, opens);
      else if (hit) void bring(hit, opens);
    },
    [reveal, loadTo, bring, choose],
  );

  /** The match after (`1`) or before (`-1`) the one gone to, round from the last to the first. */
  const step = useCallback(
    (by: 1 | -1) => {
      const { result: found, active: at } = state.current;
      const count = found.before + found.ranges.length;
      if (count === 0) return;
      // The match typing went to is in a page not loaded, a turn's closed work or a closed group
      // in it: this step goes there first.
      const current = found.ranges[at - found.before];
      if ((at >= 0 && at < found.before) || (isRange(current) && (closedFold(current) || closedGroup(current)))) goTo(at, found, true);
      else goTo((at + by + count) % count, found, true);
    },
    [goTo],
  );

  /**
   * Searches for `text`; `fresh` (the text changed) goes to the first match in the thread from
   * the view's top, or marks the newest one in the pages not loaded when the thread has none.
   */
  const run = useCallback(
    (text: string, fresh: boolean): Found => {
      const { result: last, active: at, history: pages } = state.current;
      const group = viewport?.querySelector('[data-slot="aui_message-group"]');
      const read = new Map<string, string>();
      const found = withOlder(group ? findInThread(group, idsRef.current, textOf.current, text, read) : NONE, older.current, text, pages?.oldest ?? null);
      // A message scrolled away counts its matches in what it showed.
      keepTexts.current(read);
      const { ranges, before: ahead } = found;
      const count = ahead + ranges.length;
      if (fresh) {
        // A new text: a step on its way to a match of the last one stops.
        reveals.current++;
        loading.current = null;
      }
      const load = loading.current;
      // Pages loaded for a step are not drawn yet: the bar shows what it did until they are.
      if (load && count < load.all) return found;
      // What `state` reads until the next render.
      state.current.result = found;
      setResult(found);
      if (load) {
        choose(Math.min(load.index, count - 1));
        return found;
      }
      if (count === 0) {
        choose(-1);
        return found;
      }
      if (fresh || at < 0) {
        if (ranges.length > 0) goTo(ahead + Math.max(0, firstBelow(viewport, idsRef.current, ranges)), found, false);
        else choose(ahead - 1);
        return found;
      }
      // The thread changed under the bar: the match gone to stays, where it still is. One in a
      // page not loaded keeps its index: the pages loaded since bring the matches ahead of it.
      const kept = last.ranges[at - last.before];
      const same = kept ? ranges.findIndex((hit) => sameHit(hit, kept)) : -1;
      const index = same >= 0 ? ahead + same : at < last.before ? at : at - last.before + ahead;
      choose(Math.max(0, Math.min(index, count - 1)));
      return found;
    },
    [viewport, goTo, choose],
  );

  useLayoutEffect(() => {
    rerun.current = () => void run(state.current.query, false);
    research.current = () => run(state.current.query, false);
  });

  const close = useCallback(() => {
    reveals.current++;
    scroller.mount("find", null);
    setOpen(false);
    setResult(NOTHING);
    choose(-1);
    unpaint(viewport);
    if (viewport && expanded.current.size > 0) {
      const box = viewport.getBoundingClientRect();
      const rows = [...viewport.querySelectorAll<HTMLElement>('[data-role="user"], [data-role="assistant"]')];
      const element = rows.find((row) => {
        const rect = row.getBoundingClientRect();
        return rect.top >= box.top && rect.top < box.bottom;
      }) ?? rows.find((row) => {
        const rect = row.getBoundingClientRect();
        return rect.top < box.top && rect.bottom > box.top;
      });
      if (element) closingAnchor.current = { element, top: element.getBoundingClientRect().top };
    }
    toggling.current = true;
    for (const bubble of expanded.current) {
      if (bubble.isConnected) bubble.querySelector<HTMLButtonElement>(':scope > button[aria-expanded="true"]')?.click();
    }
    toggling.current = false;
    expanded.current.clear();
    const back = before.current;
    before.current = null;
    // Where it is, without scrolling the thread there: the reader keeps their place.
    if (back instanceof HTMLElement && back.isConnected && back !== document.body) back.focus({ preventScroll: true });
  }, [viewport, scroller, choose]);

  // Collapsing may shrink a message above the reader. Keep a surviving message at its old offset,
  // and discard the scroll controller's temporary anchor on the programmatically clicked toggle.
  useLayoutEffect(() => {
    const anchor = closingAnchor.current;
    if (open || !anchor || !viewport) return;
    closingAnchor.current = null;
    if (!anchor.element.isConnected) return;
    scroller.release();
    viewport.scrollBy({ top: anchor.element.getBoundingClientRect().top - anchor.top, behavior: "instant" });
    scroller.scrolled();
  }, [open, viewport, scroller]);

  useEffect(() => {
    if (!open || !viewport) return;
    const clicked = (event: MouseEvent) => {
      if (toggling.current || !(event.target instanceof Element)) return;
      const button = event.target.closest("button");
      const bubble = button?.closest(".aui-user-message-content");
      if (bubble && bubble.querySelector(":scope > button") === button) expanded.current.delete(bubble);
    };
    viewport.addEventListener("click", clicked, true);
    return () => viewport.removeEventListener("click", clicked, true);
  }, [open, viewport]);

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

  // While the bar is open the thread's changes are searched again, a moment after they come:
  // a `<details>` opening or closing (its `open` alone changes) shows or hides its text too.
  useEffect(() => {
    if (!open || !viewport) return;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const observer = new MutationObserver(() => {
      timer ??= setTimeout(() => {
        timer = null;
        rerun.current();
      }, RERUN_MS);
    });
    observer.observe(viewport, { childList: true, subtree: true, characterData: true, attributeFilter: ["open"] });
    return () => {
      observer.disconnect();
      if (timer) clearTimeout(timer);
    };
  }, [open, viewport]);

  // A text in the bar, resting: the engine searches the pages not loaded, and the bar counts
  // their matches with the thread's own.
  const searchOlder = history?.searchOlder ?? null;
  useEffect(() => {
    if (!open || query === "" || !searchOlder) {
      older.current = null;
      return;
    }
    let current = true;
    const timer = setTimeout(() => {
      searchOlder(query, MAX_RESULTS).then(
        (found) => {
          if (!current) return;
          older.current = { query, matches: found.matches, more: found.more };
          rerun.current();
        },
        () => {},
      );
    }, OLDER_MS);
    return () => {
      current = false;
      clearTimeout(timer);
    };
  }, [open, query, searchOlder]);

  // A text in the bar: the turns' closed work mounts its rows, hidden, and is searched again then.
  const hasText = open && query !== "";
  useEffect(() => {
    if (!hasText) return;
    return startSearching();
  }, [hasText]);
  useLayoutEffect(() => {
    if (open && viewport) paint(viewport, result.ranges, result.ranges[active - result.before] ?? null);
  }, [open, viewport, result, active]);

  useLayoutEffect(() => {
    keepTexts.current = (read) => {
      if (read.size === 0) return;
      const messages = messagesNow();
      const board = useBoard.getState().board;
      for (const [id, text] of read) {
        const message = messages[placeOf(messages, id)];
        if (message) texts.current.set(id, { text, message, kept: keptChanges(rowKeyOf(threadKey, id)), board: boardRefs(message, board) });
      }
    };
    textOf.current = (id) => {
      const messages = messagesNow();
      const message = messages[placeOf(messages, id)];
      if (!message) return "";
      const kept = texts.current.get(id);
      if (kept && freshIndex(kept, message, keptChanges(rowKeyOf(threadKey, id)), useBoard.getState().board)) return kept.text;
      return wordsOf(message);
    };
  });
  // Gone with the thread: so are its marks and references to messages it opened.
  useEffect(() => {
    const opened = expanded.current;
    return () => {
      unpaint(viewport);
      opened.clear();
    };
  }, [viewport]);

  if (!open) return null;

  const count = result.before + result.ranges.length;
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
        {...PLAIN_FIELD}
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

/** What a message showed when last mounted, and what it was drawn from. */
type Indexed = { text: string; message: object; kept: number; board: readonly unknown[] };

/** The board's items a message draws from: its cards, and the workers, approvals and questions of its requests. */
function boardRefs(message: { metadata: { custom: Record<string, unknown> } }, board: ReturnType<typeof useBoard.getState>["board"]): unknown[] {
  const meta = message.metadata.custom["block"] as BlockMeta | undefined;
  if (!meta || !board) return [];
  const named = new Set([...meta.requestIds, ...meta.cards.map((card) => card.id)]);
  const refs: unknown[] = [];
  for (const items of [board.approvals, board.questions, board.tasks] as Record<string, { requestId?: string | null } | undefined>[]) {
    for (const [id, item] of Object.entries(items)) if (named.has(id) || (item?.requestId && named.has(item.requestId))) refs.push(item);
  }
  return refs;
}

/** Whether what a message showed out of sight still holds: drawn from the same message, parts and board items. */
function freshIndex(kept: Indexed, message: { metadata: { custom: Record<string, unknown> } }, changes: number, board: ReturnType<typeof useBoard.getState>["board"]): boolean {
  if (kept.message !== message || kept.kept !== changes) return false;
  const now = boardRefs(message, board);
  return now.length === kept.board.length && now.every((item, index) => item === kept.board[index]);
}

/** Whether two matches are the same one: the same place in the DOM, or the same of a message's. */
function sameHit(a: Hit, b: Hit): boolean {
  if (isRange(a) && isRange(b)) return a.startContainer === b.startContainer && a.startOffset === b.startOffset;
  return !isRange(a) && !isRange(b) && a.row === b.row && a.nth === b.nth;
}

/** The first match from the view's top down: one shown below its top, or in a message after the one there. */
function firstBelow(viewport: HTMLElement | null, ids: readonly string[], hits: readonly Hit[]): number {
  if (!viewport) return 0;
  const line = viewport.getBoundingClientRect().top;
  const rows = [...viewport.querySelectorAll<HTMLElement>('[data-slot="aui_message-group"] > [data-thread-row]')];
  const top = rows.find((row) => row.getBoundingClientRect().bottom > line);
  const topAt = top ? ids.indexOf(top.dataset["threadRow"] ?? "") : -1;
  const first = hits.findIndex((hit) => {
    if (isRange(hit)) return hit.getBoundingClientRect().bottom > line;
    return ids.indexOf(hit.row) > topAt;
  });
  return first < 0 ? 0 : first;
}
