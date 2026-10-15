import { type AssistantState, useAuiState } from "@assistant-ui/react";
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type FC,
  type PointerEvent as ReactPointerEvent,
} from "react";

import type { ThreadScroller } from "@/components/assistant-ui/thread-scroll";
import { cn } from "@/lib/utils";

/*
 * The rail of the user's messages, beside the thread: a short line per message, brighter for the
 * ones in view. Hovering one grows it and its neighbours and shows the message with the start of
 * its answer; a click scrolls its turn to the top and flashes it, a press-and-drag scrubs along
 * the thread. ⌥↑ and ⌥↓ step through the same messages from the view's top, rail or no rail.
 * Messages folded away ("N earlier messages") are on the rail too; jumping to one unfolds them,
 * and ⌥↑ past the oldest one loaded loads older pages until there is one.
 */

/** A user message the rail marks: its row's id, its text and the start of the answer after it. */
export type RailEntry = { id: string; text: string; answer: string };

/**
 * The thread's history above what it shows, as the conversation tells it: the turns folded away
 * and how to show them, and the older pages still in the database and how to load one.
 */
export type ThreadHistory = {
  /** How many turns are folded away. */
  hidden: number;
  /** The user messages folded away, oldest first. */
  folded: readonly RailEntry[];
  unfold: () => void;
  /** Older messages wait in the database. */
  hasMore: boolean;
  /** The last page asked for failed to load. */
  failed: boolean;
  /** Loads the next older page; rejects when it fails. */
  loadOlder: () => Promise<void>;
};

export const ThreadHistoryContext = createContext<ThreadHistory | null>(null);

/** The rail shows from this many user messages on. */
const MIN_MESSAGES = 4;
/** ...and while the room beside the column is at least this wide. */
const MIN_GUTTER_PX = 48;
/** A message jumped to lands with its top this far under the view's top. */
const LAND_PX = 24;
/** ⌥↑ and ⌥↓ count a message within this of the view's top as the one shown there. */
const TOLERANCE_PX = 24;
/** A message counts as in view once it is this far inside the view's edges. */
const IN_VIEW_PX = 16;
/** How much of the hovered line's growth each neighbour takes, by distance. */
const FISHEYE = [1, 0.7, 0.4, 0.2];
/** The flash on arrival: the bubble brightens by 14% and eases back. */
const FLASH = [{ filter: "brightness(1.14)" }, { filter: "brightness(1)" }];
const FLASH_MS = 600;
const FLASH_EASE = "cubic-bezier(0.23, 1, 0.32, 1)";
/** A press moves this far before it scrubs rather than clicks. */
const SCRUB_PX = 4;
/** Pages ⌥↑ loads looking for an older message before it gives up. */
const HISTORY_PAGES = 20;
/** How long a jump waits for an unfolded or loaded message to show and hold still. */
const ROW_WAIT_MS = 4000;
const STILL_FRAMES = 3;

function reducedMotion(): boolean {
  return globalThis.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
}

const nextFrame = () => new Promise<number>((resolve) => requestAnimationFrame(resolve));

/** Plain text of a message's text parts. */
function textOf(message: AssistantState["thread"]["messages"][number]): string {
  return message.content
    .map((part) => (part.type === "text" ? part.text : ""))
    .join("\n")
    .trim();
}

/** Markdown's marks dropped, runs of white space made one: enough for a preview's lines. */
function plain(text: string): string {
  return text
    .replace(/```[^\n]*\n?/g, "")
    .replace(/[*_`#>]+/g, "")
    .replace(/\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/\s+/g, " ")
    .trim()
    .slice(0, 400);
}

/** The shown user messages, as one string (it changes only when they do, not while an answer streams). */
const shownUsers = (s: AssistantState): string =>
  JSON.stringify(
    s.thread.messages.filter((message) => message.role === "user").map((message) => [message.id, plain(textOf(message)).slice(0, 200)]),
  );

/** The start of the answer after user message `id` in the shown thread. */
function answerAfter(s: AssistantState, id: string): string {
  const messages = s.thread.messages;
  const at = messages.findIndex((message) => message.id === id);
  if (at < 0) return "";
  const parts: string[] = [];
  for (const message of messages.slice(at + 1)) {
    if (message.role === "user") break;
    if (message.role === "assistant") parts.push(textOf(message));
  }
  return plain(parts.join(" "));
}

function userRows(viewport: HTMLElement): HTMLElement[] {
  return [...viewport.querySelectorAll<HTMLElement>('[data-slot="aui_message-group"] > [data-role="user"][data-message-id]')];
}

function rowOf(viewport: HTMLElement, id: string): HTMLElement | null {
  return userRows(viewport).find((row) => row.dataset["messageId"] === id) ?? null;
}

const maxScroll = (viewport: HTMLElement) => Math.max(0, viewport.scrollHeight - viewport.clientHeight);

/** Where the view's top goes for `row` to land `LAND_PX` under it, within what the view can reach. */
function landingTop(viewport: HTMLElement, row: HTMLElement): number {
  const top = viewport.scrollTop + row.getBoundingClientRect().top - viewport.getBoundingClientRect().top - LAND_PX;
  return Math.round(Math.max(0, Math.min(maxScroll(viewport), top)));
}

/** Waits until `read` gives a value, a frame at a time, for up to `ms`. */
async function waitFor<T>(read: () => T | null | undefined, ms: number, live: () => boolean): Promise<T | null> {
  const until = performance.now() + ms;
  for (;;) {
    const value = read();
    if (value !== null && value !== undefined) return value;
    if (performance.now() > until || !live()) return null;
    await nextFrame();
  }
}

/** Waits for row `id` to show and hold its place for a few frames (what opened around it settles). */
async function stillRow(viewport: HTMLElement, id: string, live: () => boolean): Promise<HTMLElement | null> {
  const until = performance.now() + ROW_WAIT_MS;
  let last: number | null = null;
  let still = 0;
  while (live()) {
    const row = rowOf(viewport, id);
    if (row) {
      const top = row.getBoundingClientRect().top;
      still = top === last ? still + 1 : 0;
      last = top;
      if (still >= STILL_FRAMES || performance.now() > until) return row;
    } else if (performance.now() > until) return null;
    await nextFrame();
  }
  return null;
}

/** Waits for a scroll to end: the view's top holds for a few frames. */
async function scrollSettled(viewport: HTMLElement, live: () => boolean): Promise<void> {
  const until = performance.now() + 2000;
  let last = viewport.scrollTop;
  let still = 0;
  while (live() && still < STILL_FRAMES && performance.now() < until) {
    await nextFrame();
    still = viewport.scrollTop === last ? still + 1 : 0;
    last = viewport.scrollTop;
  }
}

function flash(row: HTMLElement): void {
  const bubble = row.querySelector<HTMLElement>(".aui-user-message-content") ?? row;
  row.dataset["railLanded"] = String(Number(row.dataset["railLanded"] ?? 0) + 1);
  bubble.animate(FLASH, { duration: FLASH_MS, easing: FLASH_EASE });
}

/** Whether a key press is typing: in the composer, a field or anything editable. */
function typing(target: EventTarget | null): boolean {
  return target instanceof Element && !!target.closest('input, textarea, select, [contenteditable]:not([contenteditable="false"])');
}

/**
 * The rail beside the thread's column and the ⌥↑ / ⌥↓ keys. Mounted beside the thread's
 * viewport, in the thread's root.
 */
export const MessageRail: FC<{ viewport: HTMLDivElement | null; scroller: ThreadScroller }> = ({ viewport, scroller }) => {
  const history = useContext(ThreadHistoryContext);
  const shownKey = useAuiState(shownUsers);
  const shown = useMemo(
    () => (JSON.parse(shownKey) as [string, string][]).map(([id, text]) => ({ id, text })),
    [shownKey],
  );
  const folded = history?.folded;
  const entries = useMemo(
    () => [...(folded ?? []).map(({ id, text }) => ({ id, text: plain(text).slice(0, 200) })), ...shown],
    [folded, shown],
  );

  // What the jumps read after they wait: the newest, not what was there when they began.
  const historyRef = useRef(history);
  const entriesRef = useRef(entries);
  useLayoutEffect(() => {
    historyRef.current = history;
    entriesRef.current = entries;
  });
  const jumps = useRef(0);

  /** Scrolls user message `id`'s turn to the top: unfolded first when it is folded away. */
  const jumpTo = useCallback(
    async (id: string, how: { smooth: boolean; flash: boolean }) => {
      if (!viewport) return;
      const seq = ++jumps.current;
      const live = () => seq === jumps.current && viewport.isConnected;
      scroller.release();
      let row = rowOf(viewport, id);
      if (!row) {
        const folds = historyRef.current;
        if (folds?.folded.some((entry) => entry.id === id)) folds.unfold();
        row = await stillRow(viewport, id, live);
        if (!row || !live()) return;
        scroller.release();
      }
      const smooth = how.smooth && !reducedMotion();
      viewport.scrollTo({ top: landingTop(viewport, row), behavior: smooth ? "smooth" : "instant" });
      if (!how.flash) return;
      if (smooth) await scrollSettled(viewport, live);
      if (!live()) return;
      // Something above it changed size on the way: it lands where it should all the same.
      if (Math.abs(landingTop(viewport, row) - viewport.scrollTop) > 1) viewport.scrollTo({ top: landingTop(viewport, row), behavior: "instant" });
      flash(row);
    },
    [viewport, scroller],
  );

  /** The user message before the oldest one known, loading older pages until there is one. */
  const olderThanLoaded = useCallback(async (): Promise<string | null> => {
    const seq = jumps.current;
    const live = () => seq === jumps.current;
    const oldest = entriesRef.current[0]?.id ?? null;
    historyRef.current?.unfold();
    for (let page = 0; page < HISTORY_PAGES && historyRef.current?.hasMore; page++) {
      try {
        await historyRef.current.loadOlder();
      } catch {
        return null;
      }
      const found = await waitFor(
        () => {
          const all = entriesRef.current;
          const at = oldest === null ? all.length : all.findIndex((entry) => entry.id === oldest);
          return at > 0 ? all[at - 1]?.id : undefined;
        },
        1000,
        live,
      );
      if (found || !live()) return found;
    }
    return null;
  }, []);

  /** ⌥↑ / ⌥↓: the user message before or after the one at the view's top. */
  const step = useCallback(
    async (direction: -1 | 1) => {
      if (!viewport || viewport.clientHeight === 0) return;
      const line = viewport.getBoundingClientRect().top + LAND_PX;
      const rows = userRows(viewport);
      const tops = rows.map((row) => row.getBoundingClientRect().top);
      if (direction > 0) {
        const next = rows.find((_, index) => (tops[index] ?? 0) > line + TOLERANCE_PX);
        if (next?.dataset["messageId"]) await jumpTo(next.dataset["messageId"], { smooth: true, flash: true });
        return;
      }
      const previous = rows.findLast((_, index) => (tops[index] ?? 0) < line - TOLERANCE_PX);
      const id = previous?.dataset["messageId"] ?? historyRef.current?.folded.at(-1)?.id ?? (historyRef.current?.hasMore ? await olderThanLoaded() : null);
      if (id) await jumpTo(id, { smooth: true, flash: true });
    },
    [viewport, jumpTo, olderThanLoaded],
  );

  useEffect(() => {
    if (!viewport) return;
    const root = viewport.closest(".aui-thread-root");
    const onKey = (event: KeyboardEvent) => {
      if (!event.altKey || event.metaKey || event.ctrlKey || event.shiftKey || event.isComposing || event.defaultPrevented) return;
      if (event.key !== "ArrowUp" && event.key !== "ArrowDown") return;
      const target = event.target;
      const loose = target === document.body || target === document.documentElement;
      // In this thread, or with nothing focused while it is the first thread shown.
      if (!(target instanceof Node && root?.contains(target)) && !(loose && document.querySelector(".aui-thread-root") === root)) return;
      if (typing(target)) return;
      event.preventDefault();
      void step(event.key === "ArrowUp" ? -1 : 1);
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [viewport, step]);

  // The room beside the column, measured again as either resizes.
  const [gutter, setGutter] = useState(0);
  useLayoutEffect(() => {
    const column = viewport?.querySelector<HTMLElement>('[data-slot="aui_thread-column"]');
    if (!viewport || !column) return;
    const measure = () => setGutter(column.getBoundingClientRect().left - viewport.getBoundingClientRect().left);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(viewport);
    observer.observe(column);
    return () => observer.disconnect();
  }, [viewport]);

  const enabled = !!viewport && entries.length >= MIN_MESSAGES && gutter >= MIN_GUTTER_PX;
  const inView = useInView(enabled ? viewport : null);

  if (!enabled) return null;
  return <Rail entries={entries} folded={folded ?? []} inView={inView} jumpTo={jumpTo} />;
};

/** The ids of the user messages in view, as the rows come and go. */
function useInView(viewport: HTMLDivElement | null): ReadonlySet<string> {
  const [inView, setInView] = useState<ReadonlySet<string>>(() => new Set());
  useEffect(() => {
    const group = viewport?.querySelector('[data-slot="aui_message-group"]');
    if (!viewport || !group) return;
    const seen = new Map<Element, boolean>();
    const publish = () => {
      const ids = new Set<string>();
      for (const [row, visible] of seen) {
        const id = row instanceof HTMLElement ? row.dataset["messageId"] : undefined;
        if (visible && id && row.isConnected) ids.add(id);
      }
      setInView((before) => (before.size === ids.size && [...ids].every((id) => before.has(id)) ? before : ids));
    };
    const observer = new IntersectionObserver(
      (records) => {
        for (const record of records) seen.set(record.target, record.isIntersecting);
        publish();
      },
      { root: viewport, rootMargin: `${-IN_VIEW_PX}px` },
    );
    const watch = () => {
      const rows = new Set<Element>(userRows(viewport));
      for (const row of seen.keys()) {
        if (!rows.has(row)) {
          observer.unobserve(row);
          seen.delete(row);
        }
      }
      for (const row of rows) {
        if (!seen.has(row)) {
          seen.set(row, false);
          observer.observe(row);
        }
      }
      publish();
    };
    watch();
    const rows = new MutationObserver(watch);
    rows.observe(group, { childList: true });
    return () => {
      rows.disconnect();
      observer.disconnect();
    };
  }, [viewport]);
  return inView;
}

type Hover = { index: number; top: number };

const Rail: FC<{
  entries: readonly { id: string; text: string }[];
  folded: readonly RailEntry[];
  inView: ReadonlySet<string>;
  jumpTo: (id: string, how: { smooth: boolean; flash: boolean }) => Promise<void>;
}> = ({ entries, folded, inView, jumpTo }) => {
  const navRef = useRef<HTMLElement>(null);
  const listRef = useRef<HTMLOListElement>(null);
  const [hover, setHover] = useState<Hover | null>(null);
  const press = useRef<{ y: number; scrubbing: boolean; last: string | null } | null>(null);
  const scrubbed = useRef(false);

  const hoverAt = (index: number, marker: HTMLElement) => {
    const nav = navRef.current;
    if (!nav) return;
    const box = marker.getBoundingClientRect();
    setHover({ index, top: box.top + box.height / 2 - nav.getBoundingClientRect().top });
  };

  // A list longer than it shows scrolls, its ends fading (a scroll-driven animation, only then).
  const [overflows, setOverflows] = useState(false);
  useLayoutEffect(() => {
    const list = listRef.current;
    if (!list) return;
    // As the window resizes, and as markers come and go (they don't change the list's own size
    // once it is full).
    const measure = () => setOverflows(list.scrollHeight > list.clientHeight + 1);
    measure();
    const resized = new ResizeObserver(measure);
    resized.observe(list);
    const changed = new MutationObserver(measure);
    changed.observe(list, { childList: true });
    return () => {
      resized.disconnect();
      changed.disconnect();
    };
  }, []);

  // The rail's own list keeps the messages in view in sight, unless the pointer is on it.
  const firstInView = entries.findIndex((entry) => inView.has(entry.id));
  useEffect(() => {
    const list = listRef.current;
    const marker = list?.children[firstInView];
    if (!list || hover || !(marker instanceof HTMLElement)) return;
    const top = marker.offsetTop;
    if (top < list.scrollTop || top + marker.offsetHeight > list.scrollTop + list.clientHeight) {
      list.scrollTop = top - list.clientHeight / 2;
    }
  }, [firstInView, hover]);

  /** The marker under the pointer's height, while scrubbing. */
  const markerAt = (y: number): string | null => {
    const list = listRef.current;
    if (!list) return null;
    let best: { id: string; off: number } | null = null;
    for (const item of list.children) {
      const button = item.querySelector<HTMLElement>("[data-rail-marker]");
      const id = button?.dataset["railMarker"];
      if (!button || !id) continue;
      const box = button.getBoundingClientRect();
      const off = y < box.top ? box.top - y : y > box.bottom ? y - box.bottom : 0;
      if (!best || off < best.off) best = { id, off };
    }
    return best?.id ?? null;
  };

  const onPointerDown = (event: ReactPointerEvent<HTMLOListElement>) => {
    if (event.button !== 0) return;
    press.current = { y: event.clientY, scrubbing: false, last: null };
  };
  const onPointerMove = (event: ReactPointerEvent<HTMLOListElement>) => {
    const held = press.current;
    if (!held || (event.buttons & 1) === 0) return;
    if (!held.scrubbing) {
      if (Math.abs(event.clientY - held.y) < SCRUB_PX) return;
      held.scrubbing = true;
      // The drag goes on past the rail's edges; a pointer already let go can't be captured.
      try {
        event.currentTarget.setPointerCapture(event.pointerId);
      } catch {
        // Scrubs within the rail only.
      }
    }
    const id = markerAt(event.clientY);
    if (id && id !== held.last) {
      held.last = id;
      void jumpTo(id, { smooth: false, flash: false });
    }
  };
  const onPointerEnd = () => {
    scrubbed.current = press.current?.scrubbing ?? false;
    press.current = null;
  };

  const hovered = hover ? entries[hover.index] : undefined;
  const foldedEntry = hovered ? folded.find((entry) => entry.id === hovered.id) : undefined;

  return (
    <nav
      ref={navRef}
      aria-label="User messages"
      data-slot="message-rail"
      className="animate-in fade-in-0 absolute top-1/2 left-4 z-20 -translate-y-1/2 duration-150"
      onPointerLeave={() => setHover(null)}
    >
      <ol
        ref={listRef}
        className={cn(
          "hide-scrollbar max-h-rail-list relative touch-none overflow-y-auto",
          overflows && "scroll-edge-fade [--spacing-scroll-fade-bottom:var(--spacing-rail-fade)] [--spacing-scroll-fade-top:var(--spacing-rail-fade)]",
        )}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerEnd}
        onPointerCancel={onPointerEnd}
      >
        {entries.map((entry, index) => {
          const current = inView.has(entry.id);
          const grow = hover && !reducedMotion() ? (FISHEYE[Math.abs(index - hover.index)] ?? 0) : hover?.index === index ? 1 : 0;
          return (
            <li key={entry.id} className="flex">
              <button
                type="button"
                data-rail-marker={entry.id}
                aria-label={entry.text || "Message"}
                aria-current={current || undefined}
                className="w-rail-marker h-rail-marker-height flex items-center focus:outline-none"
                onPointerEnter={(event) => hoverAt(index, event.currentTarget)}
                onFocus={(event) => hoverAt(index, event.currentTarget)}
                onBlur={() => setHover(null)}
                onClick={() => {
                  if (scrubbed.current) {
                    scrubbed.current = false;
                    return;
                  }
                  void jumpTo(entry.id, { smooth: true, flash: true });
                }}
              >
                <span
                  aria-hidden
                  data-slot="rail-line"
                  className={cn(
                    "h-rail-line-height ease-rail rounded-capsule transition-[width,background-color] duration-160 motion-reduce:transition-none",
                    current || grow > 0 ? "bg-foreground/60" : "bg-foreground/40",
                  )}
                  style={{
                    width: `calc(var(--spacing-rail-line) + (var(--spacing-rail-line-hover) - var(--spacing-rail-line)) * ${grow})`,
                  }}
                />
              </button>
            </li>
          );
        })}
      </ol>
      {hover && hovered && (
        <RailPreview
          id={hovered.id}
          top={hover.top}
          text={foldedEntry ? plain(foldedEntry.text) : hovered.text}
          answer={foldedEntry ? plain(foldedEntry.answer) : null}
        />
      )}
    </nav>
  );
};

/** Beside the hovered marker: the message on one line, then the start of its answer. */
const RailPreview: FC<{ id: string; top: number; text: string; answer: string | null }> = ({ id, top, text, answer }) => {
  const shownAnswer = useAuiState((s) => (answer === null ? answerAfter(s, id) : ""));
  const reply = answer ?? shownAnswer;
  return (
    <div
      role="tooltip"
      data-slot="rail-preview"
      className="bg-popover text-popover-foreground pointer-events-none absolute left-full ms-2 flex w-80 -translate-y-1/2 flex-col gap-1 rounded-xl p-2 text-sm shadow-menu"
      style={{ top }}
    >
      {text || reply ? (
        <>
          {text && <p className="truncate font-medium">{text}</p>}
          {reply && <p className="text-muted-foreground line-clamp-3">{reply}</p>}
        </>
      ) : (
        <p className="text-muted-foreground">Preview unavailable</p>
      )}
    </div>
  );
};
