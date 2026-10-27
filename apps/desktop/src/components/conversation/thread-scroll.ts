/*
 * The thread's scrolling. The thread lays out top-down with the browser's own scroll anchoring
 * off (WebKit has none), so content growing below the view moves nothing. What does move is
 * decided here:
 * - An anchor keeps its place on screen while anything above it changes size or loads: the row
 *   crossing the view's top, or for a moment the toggle the user just clicked.
 * - Following, the bottom stays put: new content, a fold opening or closing in view and the
 *   floating composer changing size all move what is above it instead. The first turn of a
 *   thread is followed from the start; following stops when the user scrolls further than
 *   `FOLLOW_PX` from the bottom, and starts again from the scroll button, or when the user scrolls
 *   back to within `FOLLOW_PX` from further away with no more room than that below. When the turn
 *   ends, following ends too, once the turn's own last layout is followed.
 * - A message sent with the view within `PLACE_PX` of its bottom (its room aside) gets room under
 *   it: `roomFor` the view, as tall as it was when sent. The view springs to 1px from the bottom,
 *   the turn's end a third of the view down (at least 240px), and the turn's growth uses the room
 *   up, so nothing moves while its work and answer fill it; past that the answer grows below the
 *   view, not followed. The room only ever shrinks: by the turn's growth, by what the user scrolls
 *   up, to the view's new size when it or the composer resizes, and to nothing once it is all
 *   below the view, the user scrolls to the very bottom, or the content no longer needs it. Sent
 *   from further up, nothing moves: the turn is added below.
 * - While the turn works before its answer, it is watched: if it reaches past the view's bottom
 *   while its room is still more than `FOLLOW_PX`, the room goes and the turn is followed. Its
 *   final answer starting ends the watch (a turn followed stays followed).
 * - The scroll button shows once the view is more than `FOLLOW_PX` above the bottom, the room
 *   below it aside.
 * - Each conversation's place is kept for the app's run, as its distance from the bottom (the
 *   room left out), and restored on return; within `FOLLOW_PX` of it, at the bottom.
 * - While the thread is hidden (`display: none`, under a fullscreen side panel) nothing is measured
 *   or kept; shown again, it is put back where it was, or at the bottom if it was there.
 * - A jump to one of the user's messages (the rail beside the thread, ⌥↑ and ⌥↓) is the user's
 *   own move: it stops following (`release`); older history prepended above keeps the anchor.
 * - Only the messages near the view are mounted (see `thread.tsx`): a message mounting above the
 *   view at its measured height is one more change above the anchor. Code going to a message that
 *   may not be mounted asks for it first (`mount`), and finds it in place once it is.
 * - The user's scroll is told by their input: a mostly vertical wheel without Ctrl or Shift, a
 *   touch once it moved `TOUCH_SLOP_PX`, a press on the scroll element itself (its scrollbar), a
 *   scroll key outside a text field. Any wheel, touch, press or key stops a move underway.
 */

/** Within this of the bottom the thread is at its bottom: it follows, and comes back there. */
export const FOLLOW_PX = 24;
/** A message sent with the view within this of its bottom (its room aside) is placed. */
export const PLACE_PX = 300;
/** The scroll padding under the content: the floating footer's height and this (see globals.css). */
export const SCROLL_PAD_PX = 16;
/** The scroll button's glide to the bottom (easing out); within `FOLLOW_PX` it is there. */
export const TO_BOTTOM_MS = 260;
/** A sent turn's spring: critically damped, this stiff (rad/s), so it is within 0.1% in half a second. */
export const SPRING_RATE = 18.47;
/** A sent turn's spring ends this far above the bottom: the user's own scroll to the very bottom clears the room. */
const PLACED_PX = 1;
/** Scrolled up by the user to within this of the top, older messages load (see `wantsEarlier`). */
export const EARLIER_PX = 64;
/** A touch is a scroll of the user's once it moved this far. */
const TOUCH_SLOP_PX = 8;
/** A sent turn's end sits at least this far down the view, a third of it on a tall one. */
const TURN_LINE_PX = 240;
/** How long a clicked toggle keeps its place: the longest fold animation, with a frame to spare. */
const HOLD_MS = 350;
/** An edit's new message comes back from the daemon; one that comes later than this isn't placed. */
const EDIT_WINDOW_MS = 30_000;
/** A scroll this soon after the user's own input in the thread (or a jump of theirs) is theirs. */
const INPUT_MS = 1_000;
/** A scroll this soon after one of the user's is theirs too: a fling or a glide goes on by itself. */
const FLING_MS = 250;

type Rect = { readonly top: number; readonly bottom: number };

/** The scroll element, as far as the scroller reads and moves it. */
export interface ScrollBox {
  scrollTop: number;
  readonly scrollHeight: number;
  readonly clientHeight: number;
  readonly clientWidth: number;
  getBoundingClientRect(): Rect;
}

/** A row of the message list (a message's root), or a toggle in one. */
export interface RowBox {
  readonly dataset: DOMStringMap;
  readonly isConnected: boolean;
  getBoundingClientRect(): Rect;
}

/** What the scroller reads and sets in the thread (see `useThreadScroll`). */
export type ThreadLayout = {
  viewport: ScrollBox;
  /** The message rows, top to bottom. */
  rows: () => readonly RowBox[];
  /** The content's height, from the column's top to the end of the room after the rows. */
  content: () => number;
  /** The bottom padding: the floating footer's height and the gap above it. */
  padding: () => number;
  /** The floating footer's height: what hides the content under it. */
  footer: () => number;
  /** The room after the rows. */
  setRoom: (px: number) => void;
  /** Whether messages come before the rows that are mounted (none: the rows start the thread). */
  before?: () => boolean;
};

/** What the scroller reads of the user's input in the thread. */
export type ThreadInput = {
  type: string;
  target?: EventTarget | null;
  deltaX?: number;
  deltaY?: number;
  ctrlKey?: boolean;
  shiftKey?: boolean;
  key?: string;
  touches?: ArrayLike<{ clientY: number }>;
};

/** The keys that scroll the thread, outside a text field. */
const SCROLL_KEYS = new Set(["ArrowUp", "ArrowDown", "Home", "End", "PageUp", "PageDown", " "]);

/** A conversation's place while it is not shown. */
type Saved = {
  /** Its distance from the bottom, the room left out: 0 at the bottom. */
  distance: number;
  /** The row at the view's top then: mounted first when it is shown again. */
  anchor: string | null;
};
const saved = new Map<string, Saved>();

/**
 * Where conversation `key` was left, while it is away: whether at its bottom, and the message
 * at the view's top then. The thread mounts that message first, near where it is put back.
 */
export function keptPlace(key: string | null): { bottom: boolean; anchor: string | null } | null {
  const kept = key === null ? undefined : saved.get(key);
  return kept ? { bottom: kept.distance === 0, anchor: kept.anchor } : null;
}

/** Each thread's scroller, by its view: for code that moves the view on the user's behalf. */
const scrollers = new WeakMap<object, ThreadScroller>();

/**
 * Code moving a thread's view to what the user asked for (a message, a step, a report) says so
 * first, with that or the view: the move is the user's jump (see `ThreadScroller.release`),
 * where a scroll with no input of theirs is the browser's and undone while the view follows.
 */
export function jumpInThread(target: Element | null | undefined): void {
  const viewport = target?.closest("[data-slot=aui_thread-viewport]");
  if (viewport) scrollers.get(viewport)?.release();
}

/**
 * Code going to message `id` in a thread (a jump, a match, a reveal) asks for it to be mounted
 * first, as `purpose`: it is, wherever the view is, until another of that purpose is asked for
 * (null: none). It shows in its place once React has drawn it.
 */
export function mountInThread(target: Element | null | undefined, purpose: string, id: string | null): void {
  const viewport = target?.closest("[data-slot=aui_thread-viewport]") ?? target;
  if (viewport) scrollers.get(viewport)?.mount(purpose, id);
}

/**
 * A fold whose toggle isn't what should stay put (a code block's footer, which moves with its
 * end) keeps `target` in its place instead, as a click on it would (see `ThreadScroller.clicked`).
 */
export function holdInThread(target: Element | null | undefined): void {
  const viewport = target?.closest("[data-slot=aui_thread-viewport]");
  if (viewport && target) scrollers.get(viewport)?.clicked(target);
}

/**
 * Whether the scroll the thread at `viewport` just heard was the user's, upward, and ended within
 * `EARLIER_PX` of the top: older messages load. Code's scrolls (a jump, Find) never load them.
 */
export function wantsEarlier(viewport: Element): boolean {
  return scrollers.get(viewport)?.upToTop() ?? false;
}

/**
 * How many times the user acted on a thread's view, in any thread: their input in it, a focus
 * moved in, a jump, the scroll button, a message sent or an edit. Code keeping a place for the
 * user (a landed jump) stops when it changes.
 */
let actions = 0;
export function threadActions(): number {
  return actions;
}

/** Messages just sent, by conversation: their row's id, placed when it shows. */
const sentTurns = new Map<string, string>();

/**
 * The user sent the message shown as row `id` in conversation `key`: its turn is placed when
 * it shows, also in a thread not yet mounted.
 */
export function expectSentTurn(key: string, id: string): void {
  actions += 1;
  sentTurns.set(key, id);
}

/**
 * The room under a sent turn, given the view's height above the scroll padding: the turn's end
 * then sits a third of the way down (at least `TURN_LINE_PX`), or at the view's end on a short one.
 */
export function roomFor(visible: number): number {
  return Math.max(0, Math.min((visible * 2) / 3, visible - TURN_LINE_PX));
}

/**
 * Where a conversation left at distance `distance` from its bottom, with `room` under its turn
 * and `padding` of scroll padding, comes back: at the bottom (0) while more than `FOLLOW_PX` of
 * the room shows, else as far from the bottom with the room left out.
 */
export function keptDistance(distance: number, room: number, padding: number): number {
  return room - distance - padding > FOLLOW_PX ? 0 : Math.max(0, distance - room);
}

function reducedMotion(): boolean {
  return globalThis.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
}

function rowId(row: RowBox): string | undefined {
  return row.dataset["messageId"];
}

/** Whether `target` is a text field, where keys type rather than scroll. */
function typing(target: EventTarget | null | undefined): boolean {
  return typeof Element !== "undefined" && target instanceof Element && !!target.closest("input, textarea, select, [contenteditable]:not([contenteditable=false])");
}

type Anchor = { row: RowBox; offset: number };

/**
 * How the view moves with the newest turn:
 * - `static`: it doesn't; what shows stays put.
 * - `user_follow`: it stays at the bottom as content comes.
 * - `prework_watch`: the turn's work before its answer is watched; reaching past the view's
 *   bottom while more than `FOLLOW_PX` of room is left, it is followed (`prework_follow`).
 * - `prework_follow`: the work is followed; its answer starting keeps it followed (`user_follow`).
 */
export type FollowMode = "static" | "user_follow" | "prework_watch" | "prework_follow";

/**
 * The thread's scroll controller: where the view is, whether it follows new content, the room
 * under a sent turn, and each conversation's kept place.
 */
export class ThreadScroller {
  private layout: ThreadLayout | null = null;
  private key: string | null = null;
  private started = false;
  private modeNow: FollowMode = "user_follow";
  /** The newest turn's final answer started. */
  private answering = false;
  /** The turn ended while followed: following ends once this is past, its own last layout followed. */
  private followEndsAtMs: number | null = null;
  /** scrollTop as last set or seen here: a scroll to anywhere else is the user's, or the browser's. */
  private expected = 0;
  /** When the user last wheeled, touched, pressed or typed in the thread, or jumped in it. */
  private inputAtMs = Number.NEGATIVE_INFINITY;
  /** The code is moving the view for the user (`release`), and its scroll isn't heard yet. */
  private jumped = false;
  /** A pointer is held down on the scroll element (dragging its scrollbar). */
  private pressed = false;
  /** Where a touch started, and whether it moved far enough to be a scroll. */
  private touchY: number | null = null;
  private touchMoved = false;
  /** When a scroll last was the user's. */
  private userScrollAtMs = Number.NEGATIVE_INFINITY;
  /** The scroll last heard was the user's, upward. */
  private userUp = false;
  private lastDistance = 0;
  /** The view's size as last handled, and whether the view was at its bottom then. */
  private size = { width: 0, height: 0 };
  private atBottom = true;
  /** The content's height as `resized` last heard it. */
  private contentHeight = 0;
  private paddingPx = 0;
  private anchor: Anchor | null = null;
  /** The toggle just clicked, kept where it was until its fold settles. */
  private held: (Anchor & { untilMs: number }) | null = null;
  /** The distance from the bottom asked for on showing: applied again as content loads, until reached or the user acts. */
  private restore: number | null = null;
  /** The user message whose turn has room, and the room's height. */
  private room: string | null = null;
  private roomPx = 0;
  /** The room's turn's height as last measured, from its user message to the last row's end. */
  private roomTurn: number | null = null;
  /** The thread works on its newest turn. */
  private running = false;
  /** The view is further from the bottom than the room and `FOLLOW_PX`: the scroll button shows. */
  private below = false;
  private newestUser: string | null = null;
  private editSentAtMs: number | null = null;
  private glide: number | null = null;
  /** Counts glides started and stopped: a frame of one stopped does nothing. */
  private glides = 0;
  /** The glide underway is a sent turn's spring: a resize starts it again. */
  private springing = false;
  /** The view was hidden, and its showing again isn't handled yet. */
  private hidden = false;
  private readonly listeners = new Set<() => void>();
  private mounter: ((purpose: string, id: string | null) => void) | null = null;

  get following(): boolean {
    return this.follow;
  }

  get mode(): FollowMode {
    return this.modeNow;
  }

  /** New content at the end is followed: the view stays at the bottom. */
  private get follow(): boolean {
    return this.modeNow === "user_follow" || this.modeNow === "prework_follow";
  }

  get distance(): number {
    return this.lastDistance;
  }

  /** The room under the newest turn. */
  get roomHeight(): number {
    return this.roomPx;
  }

  /** Whether the view is further from the bottom than the room and `FOLLOW_PX`: the scroll button shows. */
  contentBelow = (): boolean => this.below;

  /** Whether the scroll last heard was the user's, upward, to within `EARLIER_PX` of the top. */
  upToTop(): boolean {
    const viewport = this.layout?.viewport;
    return this.userUp && !!viewport && viewport.scrollTop <= EARLIER_PX;
  }

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  /** The message list mounts what is asked for with `mount` (null: nothing does). */
  setMounter(mounter: ((purpose: string, id: string | null) => void) | null): void {
    this.mounter = mounter;
  }

  /** Message `id` is mounted as `purpose`, wherever the view is, until another of that purpose. */
  mount = (purpose: string, id: string | null): void => {
    this.mounter?.(purpose, id);
  };

  /** Shows conversation `key` (null: nothing kept) in `layout`, until the returned call. */
  attach(layout: ThreadLayout, key: string | null): () => void {
    this.stopGlide();
    this.layout = layout;
    this.key = key;
    this.started = false;
    this.hidden = false;
    this.size = { width: layout.viewport.clientWidth, height: layout.viewport.clientHeight };
    this.paddingPx = layout.padding();
    this.modeNow = "user_follow";
    this.answering = false;
    this.followEndsAtMs = null;
    this.anchor = null;
    this.held = null;
    this.restore = null;
    this.dropRoom();
    this.roomPx = 0;
    this.newestUser = null;
    this.lastDistance = 0;
    this.below = false;
    layout.setRoom(0);
    scrollers.set(layout.viewport, this);
    this.rowsChanged();
    return () => {
      this.stopGlide();
      if (this.layout === layout) this.layout = null;
      if (scrollers.get(layout.viewport) === this) scrollers.delete(layout.viewport);
    };
  }

  /* --- what the thread tells it ----------------------------------------------------------- */

  /** Rows were added, removed or replaced. */
  rowsChanged(): void {
    const layout = this.layout;
    if (!layout) return;
    const rows = layout.rows();
    const previous = this.newestUser;
    const newest = newestUserRow(rows);
    this.newestUser = newest ? (rowId(newest) ?? null) : null;
    if (!this.started) {
      if (rows.length > 0 && !this.hide(layout.viewport)) this.start();
      return;
    }
    if (this.newestUser !== previous) this.answering = false;
    if (this.newestUser !== null && this.newestUser !== previous) {
      const sent = this.key !== null && sentTurns.get(this.key) === this.newestUser;
      const edited = this.editSentAtMs !== null && performance.now() - this.editSentAtMs < EDIT_WINDOW_MS;
      if (sent || edited) {
        if (sent && this.key !== null) sentTurns.delete(this.key);
        this.editSentAtMs = null;
        // The thread's first turn is followed from the bottom, with no room.
        if (this.firstTurn(rows)) {
          this.interrupt();
          this.followFromHere();
          this.resized();
          return;
        }
        // Sent from further up, reading, the user stays where they are: the turn is added below.
        // The distance is the view's before the new row came.
        if (this.lastDistance - this.roomPx > PLACE_PX) {
          this.modeNow = "static";
          this.clearRoom();
          this.resized();
          return;
        }
        this.place(this.newestUser);
        return;
      }
    }
    this.resized();
  }

  /** The user sent an edit of one of their messages: its new message's turn is placed. */
  editSent = (): void => {
    actions += 1;
    this.editSentAtMs = performance.now();
  };

  /**
   * Whether the thread works on its newest turn. Work starting with the view static is watched
   * (a sent turn's, or work that starts again with no message: a plan approved, a question
   * answered). A turn ending stops the watch; followed, it is followed until its own last layout
   * is (`HOLD_MS`), and then not.
   */
  setRunning(running: boolean): void {
    const resumed = running && !this.running;
    const ended = !running && this.running;
    // Growth not heard of yet is handled while the watch still holds: the last of a turn comes
    // in the same update as its end, and a window that paints nothing (in the background) hears
    // of no resize at all.
    if (!running && this.running && this.modeNow !== "static") this.resized();
    this.running = running;
    if (resumed) {
      this.answering = false;
      this.followEndsAtMs = null;
      if (this.started) this.workStarted();
    }
    if (ended) this.turnEnded();
  }

  /**
   * The final answer of the turn answering `requestIds` started: if that is the newest turn, its
   * followed work stays followed (`user_follow`) and its watched work is not watched any more
   * (`static`); a scroll away from here stops following outright. An older turn's says nothing.
   */
  answerStarted(requestIds: readonly string[]): void {
    if (this.answering || this.newestUser === null || !requestIds.includes(this.newestUser)) return;
    this.answering = true;
    if (this.modeNow === "prework_follow") this.modeNow = "user_follow";
    else if (this.modeNow === "prework_watch") this.modeNow = "static";
  }

  /**
   * Something changed size: the view, the content, or the floating footer. The view stays on
   * what it shows, unless it follows new content or was at the bottom of a view that resized.
   */
  resized(): void {
    const layout = this.layout;
    if (!layout) return;
    const viewport = layout.viewport;
    if (this.hide(viewport)) return;
    if (!this.started) {
      if (layout.rows().length > 0) this.start();
      return;
    }
    this.endFollowIfOver();
    // Shown again: the browser may have reset the view's top while it was hidden.
    const shown = this.hidden;
    this.hidden = false;
    // A jump's move made in a frame callback is heard as a scroll only with the next frame, after
    // a resize in this one: the view is where the jump put it, not at the anchor from before.
    if (this.jumped && Math.abs(viewport.scrollTop - this.expected) >= 1) this.scrolled();
    const wasAtBottom = this.atBottom;
    const viewChanged =
      viewport.clientWidth !== this.size.width ||
      viewport.clientHeight !== this.size.height ||
      layout.padding() !== this.paddingPx;
    this.size = { width: viewport.clientWidth, height: viewport.clientHeight };
    this.paddingPx = layout.padding();
    // A smaller view or a taller composer trims the room to the size it would be given now.
    if (viewChanged) this.trimRoom();
    const roomBefore = this.roomPx;
    this.updateRoom();
    // Kept only here: a frame runs its scroll events before its resize observers, so a scroll of
    // the view's own heard after the content grew would take in the growth this should follow.
    const contentChanged = layout.content() !== this.contentHeight;
    this.contentHeight = layout.content();
    if (this.glide !== null) {
      // A sent turn's spring starts again from where it is, to the new bottom.
      if (viewChanged && this.springing) this.spring();
      this.record();
      return;
    }
    const held = this.held;
    if (held && (performance.now() > held.untilMs || !held.row.isConnected)) this.held = null;
    if (this.restore !== null) this.applyRestore();
    else if (this.held) this.keep(this.held);
    else if (((viewChanged || shown) && wasAtBottom) || (this.follow && (contentChanged || viewChanged || shown))) {
      this.toBottom();
      this.record();
      return;
    } else if (this.modeNow === "prework_watch" && roomBefore > FOLLOW_PX && this.overflows()) {
      // The turn's work reached past the view's bottom with room left: from here it is followed.
      this.modeNow = "prework_follow";
      this.clearRoom();
      this.toBottom();
      this.record();
      return;
    } else if (this.anchor) this.keep(this.anchor);
    this.record(false);
  }

  /**
   * A scroll event. One that didn't come from here is the user's when it comes with their input
   * (a wheel, a touch, a held scrollbar, a key, a focus moved in, a jump) or right after a scroll
   * of theirs (a fling going on); any other is the browser's: clamping the view to a range that
   * shrank under it (a turn re-laid out as its work starts again, say, its content back before
   * the scroll is heard), hiding or showing it. The browser's moves nothing: a view that follows
   * goes back to its bottom.
   */
  scrolled(): void {
    const viewport = this.layout?.viewport;
    if (!viewport || this.hide(viewport)) return;
    if (this.hidden) {
      this.resized();
      return;
    }
    this.endFollowIfOver();
    // A jump's scroll is the user's, but code's: it moves no history in (`wantsEarlier`).
    const jump = this.jumped;
    this.jumped = false;
    this.userUp = false;
    const max = maxScroll(viewport);
    const clamped = this.expected > max && Math.abs(viewport.scrollTop - max) < 1;
    if (clamped || Math.abs(viewport.scrollTop - this.expected) < 1) {
      // Clamped, the view's top moved: its anchor is where it is now.
      this.record(clamped);
      return;
    }
    const now = performance.now();
    if (!this.pressed && now - this.inputAtMs > INPUT_MS && now - this.userScrollAtMs > FLING_MS) {
      if (this.follow && this.glide === null) this.toBottom();
      else this.clearRoomIfUnneeded();
      this.record();
      return;
    }
    this.userScrollAtMs = now;
    this.interrupt();
    const before = this.lastDistance;
    const up = viewport.scrollTop < this.expected;
    this.userUp = up && !jump;
    // Scrolled up, the room shrinks by as much, and goes once it is all below the view.
    if (up && this.roomPx > 0) {
      this.setRoomPx(Math.max(0, this.roomPx - (this.expected - viewport.scrollTop)));
      if (this.roomPx === 0 || this.roomHidden()) this.clearRoom();
    }
    // At the very bottom, the room goes.
    if (this.roomPx > 0 && maxScroll(viewport) - viewport.scrollTop < 0.5) this.clearRoom();
    const distance = maxScroll(viewport) - viewport.scrollTop;
    if (up) {
      if (distance > FOLLOW_PX && this.follow) {
        // Away from the bottom: nothing follows; work before the answer is watched again.
        this.followEndsAtMs = null;
        this.modeNow = this.running && !this.answering ? "prework_watch" : "static";
      }
    } else if (distance <= FOLLOW_PX && before > FOLLOW_PX && this.roomPx <= FOLLOW_PX) {
      this.followFromHere();
    }
    this.record();
  }

  /** The user scrolls, presses, clicks or types in the thread: what was underway stops. */
  interrupt = (): void => {
    this.stopGlide();
    this.restore = null;
    this.held = null;
  };

  /**
   * The user's input in the thread (a wheel, a touch, a key, a pointer pressed or let go): what
   * was underway stops, and the scrolls that come with it are theirs when it is one that scrolls.
   */
  input = (event?: ThreadInput): void => {
    actions += 1;
    const type = event?.type;
    if (type === "pointerup" || type === "pointercancel") {
      if (this.pressed) this.inputAtMs = performance.now();
      this.pressed = false;
      return;
    }
    this.interrupt();
    if (this.scrolls(event)) this.inputAtMs = performance.now();
  };

  /** Whether input `event` scrolls the view as the user's own (see the top of this file). */
  private scrolls(event: ThreadInput | undefined): boolean {
    switch (event?.type) {
      case undefined:
        return true;
      case "wheel":
        return Math.abs(event.deltaY ?? 1) > Math.abs(event.deltaX ?? 0) && !event.ctrlKey && !event.shiftKey;
      case "touchstart":
        this.touchY = event.touches?.[0]?.clientY ?? null;
        this.touchMoved = false;
        return false;
      case "touchmove": {
        const y = event.touches?.[0]?.clientY;
        if (this.touchY === null || y === undefined || Math.abs(y - this.touchY) > TOUCH_SLOP_PX) this.touchMoved = true;
        return this.touchMoved;
      }
      case "touchend":
      case "touchcancel": {
        const moved = this.touchMoved;
        this.touchY = null;
        this.touchMoved = false;
        return moved;
      }
      case "pointerdown":
        // Only on the scroll element itself, its scrollbar: a press on content selects it.
        this.pressed = event.target === undefined || (event.target as unknown) === this.layout?.viewport;
        return this.pressed;
      case "keydown":
        return !typing(event.target) && SCROLL_KEYS.has(event.key ?? "");
      default:
        return true;
    }
  }

  /**
   * The focus moved into the thread (Tab from the composer, the latest response's pill, a link):
   * the browser's scroll that brings it into view is the user's.
   */
  focused = (): void => {
    actions += 1;
    this.inputAtMs = performance.now();
  };

  /**
   * The user jumps to a place themselves (one of their messages, from the rail or ⌥↑/⌥↓): as
   * with their own scroll away from the bottom, nothing follows, springs or is watched. Reaching
   * the bottom again follows again (see `scrolled`).
   */
  release = (): void => {
    actions += 1;
    this.inputAtMs = performance.now();
    this.jumped = true;
    this.interrupt();
    this.modeNow = "static";
    this.followEndsAtMs = null;
  };

  /**
   * A click in the thread: a button or toggle keeps its place while what it opens or closes
   * settles, unless the view follows at its bottom, which stays put. Opening or closing a fold
   * stops following, and the watch of a sent turn.
   */
  clicked(target: Element): void {
    const layout = this.layout;
    const toggle = target.closest<HTMLElement>("button, summary, [role=button], [aria-expanded]");
    if (!layout || !toggle) return;
    const fold = toggle.matches("summary, [aria-expanded]");
    if (fold && this.modeNow === "prework_watch") this.modeNow = "static";
    if (this.follow && this.roomPx === 0 && this.lastDistance <= FOLLOW_PX) return;
    const viewTop = layout.viewport.getBoundingClientRect().top;
    const top = toggle.getBoundingClientRect().top;
    this.held = { row: toggle, offset: top - viewTop, untilMs: performance.now() + HOLD_MS };
    if (fold) {
      this.modeNow = "static";
      this.followEndsAtMs = null;
    }
  }

  /**
   * The scroll button: to the bottom, following from there. While the thread works, or with
   * more than `FOLLOW_PX` of room, the room goes and it is there at once; otherwise it glides,
   * there once within `FOLLOW_PX`.
   */
  scrollToBottom = (): void => {
    const layout = this.layout;
    if (!layout) return;
    actions += 1;
    this.interrupt();
    this.followFromHere();
    if (this.running || this.roomPx > FOLLOW_PX) {
      this.clearRoom();
      this.toBottom();
      this.record();
      return;
    }
    this.glideTo(() => maxScroll(layout.viewport), easeOut, FOLLOW_PX);
  };

  /* --- placing ---------------------------------------------------------------------------- */

  /** The first rows show: back to where the user left, or the bottom. */
  private start(): void {
    const layout = this.layout;
    if (!layout) return;
    this.started = true;
    // It starts shown: a thread attached hidden is measured from here.
    this.hidden = false;
    this.size = { width: layout.viewport.clientWidth, height: layout.viewport.clientHeight };
    this.paddingPx = layout.padding();
    const key = this.key;
    if (key !== null && this.newestUser !== null && sentTurns.get(key) === this.newestUser) {
      sentTurns.delete(key);
      // The thread's first turn (sent from the new-chat view) is followed from the bottom.
      if (!this.firstTurn(layout.rows())) {
        this.place(this.newestUser);
        return;
      }
    } else {
      const kept = key === null ? undefined : saved.get(key);
      if (kept && kept.distance > 0) {
        this.modeNow = "static";
        this.restore = kept.distance;
        this.applyRestore();
        this.record(false);
        return;
      }
    }
    this.followFromHere();
    this.clearRoom();
    this.toBottom();
    this.record();
  }

  /**
   * Puts the view back at its kept distance from the bottom; once that is in range and rows show
   * there (the list mounts them for where the view is), the anchor keeps it.
   */
  private applyRestore(): void {
    const restore = this.restore;
    const layout = this.layout;
    if (restore === null || !layout) return;
    const viewport = layout.viewport;
    const target = maxScroll(viewport) - restore;
    this.setTop(target);
    const view = viewport.getBoundingClientRect();
    const shows = layout.rows().some((row) => {
      const rect = row.getBoundingClientRect();
      return rect.bottom > view.top && rect.top < view.bottom;
    });
    if (target >= 0 && shows) this.restore = null;
  }

  /** Gives the turn of user message `id` its room and springs to the bottom, watching it. */
  private place(id: string): void {
    const layout = this.layout;
    if (!layout) return;
    this.interrupt();
    // Placed, the view holds still; its turn's work, already started or starting, is watched.
    this.modeNow = "static";
    this.followEndsAtMs = null;
    this.answering = false;
    if (this.running) this.workStarted();
    this.dropRoom();
    this.room = id;
    this.setRoomPx(roomFor(this.visible()));
    this.updateRoom();
    this.spring();
    this.record();
  }

  /** The spring to the placed turn's bottom, from where the view is. */
  private spring(): void {
    const viewport = this.layout?.viewport;
    if (!viewport) return;
    this.glideTo(() => Math.max(0, maxScroll(viewport) - PLACED_PX), spring);
    this.springing = this.glide !== null;
  }

  /** Follows from here: the view stays at the bottom as content comes. */
  private followFromHere(): void {
    this.modeNow = "user_follow";
    this.followEndsAtMs = null;
  }

  /** The newest turn's work starts: from a still view, it is watched. */
  private workStarted(): void {
    if (this.modeNow === "static") this.modeNow = "prework_watch";
  }

  /** The turn ended while followed and its own last layout is followed: from here nothing is. */
  private endFollowIfOver(): void {
    if (this.followEndsAtMs === null || performance.now() < this.followEndsAtMs) return;
    this.followEndsAtMs = null;
    this.modeNow = "static";
  }

  /** Whether the newest user row starts the thread: its turn is the first. */
  private firstTurn(rows: readonly RowBox[]): boolean {
    return rows.filter((row) => row.dataset["role"] === "user").length === 1 && !(this.layout?.before?.() ?? false);
  }

  /** The view's height above the scroll padding. */
  private visible(): number {
    const layout = this.layout;
    return layout ? layout.viewport.clientHeight - layout.footer() - SCROLL_PAD_PX : 0;
  }

  /** The room's turn's growth since it was last measured uses the room up; at 0 it is gone. */
  private updateRoom(): void {
    const layout = this.layout;
    if (!layout || this.hide(layout.viewport) || this.room === null) return;
    const rows = layout.rows();
    const user = newestUserRow(rows);
    const last = rows.at(-1);
    if (!user || !last || rowId(user) !== this.room) {
      this.clearRoom();
      return;
    }
    const turn = last.getBoundingClientRect().bottom - user.getBoundingClientRect().top;
    if (this.roomTurn !== null && turn > this.roomTurn) this.setRoomPx(Math.max(0, this.roomPx - (turn - this.roomTurn)));
    this.roomTurn = turn;
    if (this.roomPx === 0) this.dropRoom();
  }

  /** The room no more than what the view would give a sent turn now. */
  private trimRoom(): void {
    if (this.roomPx === 0) return;
    this.setRoomPx(Math.min(this.roomPx, roomFor(this.visible())));
    if (this.roomPx === 0) this.dropRoom();
  }

  /** Whether the room is all below the view. */
  private roomHidden(): boolean {
    const layout = this.layout;
    const last = layout?.rows().at(-1);
    if (!layout || !last) return false;
    return last.getBoundingClientRect().bottom >= layout.viewport.getBoundingClientRect().bottom;
  }

  /** At the very bottom with content that fits the view without it, the room goes. */
  private clearRoomIfUnneeded(): void {
    const viewport = this.layout?.viewport;
    if (!viewport || this.roomPx === 0) return;
    const atEnd = maxScroll(viewport) - viewport.scrollTop < 0.5;
    if (atEnd && viewport.scrollHeight - this.roomPx <= viewport.clientHeight) this.clearRoom();
  }

  /** The newest turn ended: it is no longer watched; followed, it is until its last layout is. */
  private turnEnded(): void {
    if (this.modeNow === "prework_watch") this.modeNow = "static";
    if (this.follow) this.followEndsAtMs = performance.now() + HOLD_MS;
  }

  private dropRoom(): void {
    this.room = null;
    this.roomTurn = null;
  }

  private clearRoom(): void {
    this.dropRoom();
    this.setRoomPx(0);
  }

  private setRoomPx(px: number): void {
    if (px === this.roomPx) return;
    this.roomPx = px;
    this.layout?.setRoom(px);
  }

  /** Whether the newest row reaches past the view's bottom, above the scroll padding. */
  private overflows(): boolean {
    const layout = this.layout;
    const last = layout?.rows().at(-1);
    if (!layout || !last) return false;
    const end = layout.viewport.getBoundingClientRect().top + this.visible();
    return last.getBoundingClientRect().bottom > end + 1;
  }

  /* --- moving ----------------------------------------------------------------------------- */

  /** Whether the view is hidden (it has no height): it is noted, and nothing is measured. */
  private hide(viewport: ScrollBox): boolean {
    if (viewport.clientHeight > 0) return false;
    this.hidden = true;
    return true;
  }

  private offsetOf(row: RowBox): number {
    const viewport = this.layout?.viewport;
    return viewport ? row.getBoundingClientRect().top - viewport.getBoundingClientRect().top : 0;
  }

  private setTop(top: number): void {
    const viewport = this.layout?.viewport;
    if (!viewport) return;
    viewport.scrollTop = top;
    this.expected = viewport.scrollTop;
  }

  private toBottom(): void {
    const viewport = this.layout?.viewport;
    if (viewport) this.setTop(maxScroll(viewport));
  }

  /** Puts `anchor` back at its offset from the view's top. */
  private keep(anchor: Anchor): void {
    const viewport = this.layout?.viewport;
    if (!viewport || !anchor.row.isConnected) return;
    const moved = this.offsetOf(anchor.row) - anchor.offset;
    if (moved !== 0) this.setTop(viewport.scrollTop + moved);
  }

  /**
   * Moves the view to `target` along `curve`: the share of the way still to go after `ms`, done
   * at 0, or once within `snapPx` of it. The target is read again each frame, as content changes;
   * when it moves, the way left is rescaled from where the view is, so the view never jumps or
   * turns back (a placeholder under a sent message giving way to its reply makes the turn shorter
   * mid-spring).
   */
  private glideTo(target: () => number, curve: (ms: number) => number, snapPx = 0): void {
    const viewport = this.layout?.viewport;
    if (!viewport) return;
    this.stopGlide();
    if (reducedMotion()) {
      this.setTop(target());
      return;
    }
    let goal = target();
    let offset = goal - viewport.scrollTop;
    let left = 1;
    let begun: number | null = null;
    const run = this.glides;
    const step = (now: number) => {
      if (run !== this.glides) return;
      if (!this.layout) {
        this.glide = null;
        return;
      }
      // Hidden, it waits to be shown and then ends where it was going.
      if (this.hide(this.layout.viewport)) {
        this.glide = requestAnimationFrame(step);
        return;
      }
      begun ??= now;
      const to = target();
      if (to !== goal) offset += (to - goal) / left;
      goal = to;
      left = curve(now - begun);
      const way = Math.abs(offset * left);
      const done = way < 0.5 || way <= snapPx;
      this.setTop(goal - (done ? 0 : offset * left));
      this.glide = done ? null : requestAnimationFrame(step);
      if (done) this.springing = false;
      this.record();
    };
    this.glide = requestAnimationFrame(step);
  }

  private stopGlide(): void {
    if (this.glide !== null) cancelAnimationFrame(this.glide);
    this.glide = null;
    this.glides += 1;
    this.springing = false;
  }

  /**
   * Notes where the view is now: its distance, its anchor, and the conversation's kept place.
   * Unless `measured`, an anchor still at the view's top keeps the offset it was asked to keep,
   * so whole-pixel scrolling under fractional rows doesn't drift it a pixel at a time.
   */
  private record(measured = true): void {
    const layout = this.layout;
    if (!layout || this.hide(layout.viewport)) return;
    const viewport = layout.viewport;
    this.expected = viewport.scrollTop;
    this.lastDistance = Math.max(0, maxScroll(viewport) - viewport.scrollTop);
    // A scroll can come between the view resizing and `resized` hearing of it: where the view
    // was is kept for the size it was.
    if (viewport.clientWidth === this.size.width && viewport.clientHeight === this.size.height) {
      this.atBottom = this.lastDistance <= FOLLOW_PX;
    }
    this.below = this.lastDistance > this.roomPx + FOLLOW_PX;
    const rows = layout.rows();
    const viewTop = viewport.getBoundingClientRect().top;
    // The first row whose bottom is below the view's top: the rows are in order.
    let low = 0;
    let high = rows.length;
    while (low < high) {
      const middle = (low + high) >> 1;
      if ((rows[middle]?.getBoundingClientRect().bottom ?? 0) > viewTop) high = middle;
      else low = middle + 1;
    }
    const row = rows[Math.min(low, rows.length - 1)];
    if (measured || this.anchor?.row !== row) {
      this.anchor = row ? { row, offset: row.getBoundingClientRect().top - viewTop } : null;
    }
    if (this.started && this.key !== null) {
      const id = row ? rowId(row) : undefined;
      // A place still being restored is the place; following, the bottom.
      const kept = keptDistance(this.lastDistance, this.roomPx, layout.footer() + SCROLL_PAD_PX);
      const distance = this.restore ?? (this.follow || kept <= FOLLOW_PX ? 0 : kept);
      saved.set(this.key, { distance, anchor: id ?? null });
    }
    for (const listener of this.listeners) listener();
  }
}

/** The share left of a sent turn's move after `ms`: a critically damped spring. */
function spring(ms: number): number {
  const t = (SPRING_RATE * ms) / 1000;
  return (1 + t) * Math.exp(-t);
}

/** The share left of the scroll button's glide after `ms`: easing out. */
function easeOut(ms: number): number {
  return (1 - Math.min(1, ms / TO_BOTTOM_MS)) ** 3;
}

function maxScroll(viewport: ScrollBox): number {
  return Math.max(0, viewport.scrollHeight - viewport.clientHeight);
}

function newestUserRow(rows: readonly RowBox[]): RowBox | undefined {
  return rows.findLast((row) => row.dataset["role"] === "user");
}
