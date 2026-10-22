/*
 * The thread's scrolling. The thread lays out top-down with the browser's own scroll anchoring
 * off (WebKit has none), so content growing below the view moves nothing. What does move is
 * decided here:
 * - An anchor keeps its place on screen while anything above it changes size or loads: the row
 *   crossing the view's top, or for a moment the toggle the user just clicked.
 * - At the bottom, following, the bottom stays put: new content, a fold opening or closing in
 *   view and the floating composer changing size all move what is above it instead. Following
 *   starts when the user scrolls down to within `FOLLOW_PX` of the bottom or takes the scroll
 *   button, and stops when they scroll further away than that.
 * - A sent message's turn springs up so its end sits a third of the view down (at least 240px),
 *   with room below it: the turn's growth uses the room up, so nothing moves while its work and
 *   answer fill it. Once the running turn reaches past the view's bottom it is followed, unless
 *   the user scrolled away or opened a fold in the meantime. The room only ever shrinks: by the
 *   turn's growth, and by what the user scrolls below the view; a turn ending leaves what shows.
 *   Sent while the user had scrolled up, reading, nothing moves: the turn is added below. Work
 *   starting again with no message (a plan approved, a question answered) is watched the same way.
 *   A turn ending is measured before its watch ends, so growth no resize told of is followed.
 * - The scroll button shows while a row reaches under the floating footer or below the view,
 *   never for the room alone or for content in the gap above the footer, which stays in view.
 * - Each conversation's place is kept for the app's run, by row id, and restored on return: a
 *   conversation left at its bottom (its sent turn's room in view, too) comes back at the bottom.
 * - While the thread is hidden (`display: none`, under a fullscreen side panel) nothing is measured
 *   or kept; shown again, it is put back where it was, or at the bottom if it was there.
 * - A jump to one of the user's messages (the rail beside the thread, ⌥↑ and ⌥↓) is the user's
 *   own move: it stops following (`release`); older history prepended above keeps the anchor.
 */

/** Within this of the bottom the thread is at its bottom: it follows, and comes back there. */
export const FOLLOW_PX = 24;
/** How far a row may reach under the floating footer before the scroll button shows. */
export const BELOW_PX = 8;
/** The scroll button's glide to the bottom (easing out). */
const TO_BOTTOM_MS = 260;
/** A sent turn's spring: critically damped, this stiff (rad/s), so it settles in about half a second. */
const SPRING_RATE = 17.5;
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
};

/** A conversation's place while it is not shown. */
type Saved = {
  /** It was at its bottom: it comes back there, following. */
  bottom: boolean;
  /** The row at the view's top and its top's offset from the view's top. */
  anchor: { id: string; offset: number } | null;
  /** The rows' ids then, in order: an anchor that is gone falls back to the nearest one before it. */
  ids: readonly string[];
  /** The room in view under a sent turn, kept so the place can be reached again. */
  room: { id: string; px: number; turn: number } | null;
};
const saved = new Map<string, Saved>();

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
 * A fold whose toggle isn't what should stay put (a code block's footer, which moves with its
 * end) keeps `target` in its place instead, as a click on it would (see `ThreadScroller.clicked`).
 */
export function holdInThread(target: Element | null | undefined): void {
  const viewport = target?.closest("[data-slot=aui_thread-viewport]");
  if (viewport && target) scrollers.get(viewport)?.clicked(target);
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
 * it shows, also in a thread not yet mounted (the first message, sent from the new-chat view).
 */
export function expectSentTurn(key: string, id: string): void {
  actions += 1;
  sentTurns.set(key, id);
}

/**
 * The room under a sent turn, given the view's height above the footer's gap: the turn's end
 * then sits a third of the way down (at least `TURN_LINE_PX`), or at the view's end on a short one.
 */
export function roomFor(visible: number): number {
  return Math.round(Math.max(0, Math.min((visible * 2) / 3, visible - TURN_LINE_PX)));
}

function reducedMotion(): boolean {
  return globalThis.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
}

function rowId(row: RowBox): string | undefined {
  return row.dataset["messageId"];
}

type Anchor = { row: RowBox; offset: number };

/**
 * The thread's scroll controller: where the view is, whether it follows new content, the room
 * under a sent turn, and each conversation's kept place.
 */
export class ThreadScroller {
  private layout: ThreadLayout | null = null;
  private key: string | null = null;
  private started = false;
  /** New content at the end is followed: the view stays at the bottom. */
  private follow = true;
  /** A sent turn is watched: once it reaches past the view's bottom, it is followed. */
  private watch = false;
  /** The user scrolled away from the bottom since they were last there or sent a message. */
  private away = false;
  /** scrollTop as last set or seen here: a scroll to anywhere else is the user's, or the browser's. */
  private expected = 0;
  /** When the user last wheeled, touched, pressed or typed in the thread, or jumped in it. */
  private inputAtMs = Number.NEGATIVE_INFINITY;
  /** The code is moving the view for the user (`release`), and its scroll isn't heard yet. */
  private jumped = false;
  /** A pointer is held down in the thread (dragging the scrollbar, selecting text). */
  private pressed = false;
  /** When a scroll last was the user's. */
  private userScrollAtMs = Number.NEGATIVE_INFINITY;
  private lastDistance = 0;
  /** The view's size as last handled, and whether the view was at its bottom then. */
  private size = { width: 0, height: 0 };
  private atBottom = true;
  private contentHeight = 0;
  private paddingPx = 0;
  private anchor: Anchor | null = null;
  /** The toggle just clicked, kept where it was until its fold settles. */
  private held: (Anchor & { untilMs: number }) | null = null;
  /** The place asked for on showing: applied again as content loads, until reached or the user acts. */
  private restore: Anchor | null = null;
  /** The user message whose turn has room, and the room's height. */
  private room: string | null = null;
  private roomPx = 0;
  /** What of the room is used up: by the turn growing, and by the user scrolling it out of view. */
  private roomUsed = 0;
  /** The room's turn's height as last measured, from its user message to the last row's end. */
  private roomTurn: number | null = null;
  /** The sent turn was seen running: its end is the watch's end. */
  private ran = false;
  /** The thread works on its newest turn. */
  private running = false;
  /** A row reaches under the floating footer: some of it is hidden. */
  private below = false;
  private ids: readonly string[] = [];
  private newestUser: string | null = null;
  private editSentAtMs: number | null = null;
  private glide: number | null = null;
  /** The view was hidden, and its showing again isn't handled yet. */
  private hidden = false;
  private readonly listeners = new Set<() => void>();

  get following(): boolean {
    return this.follow;
  }

  get distance(): number {
    return this.lastDistance;
  }

  /**
   * Whether a row is hidden under the footer or below the view (the room and the gap above the
   * footer don't count): the scroll button shows.
   */
  contentBelow = (): boolean => this.below;

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
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
    this.follow = true;
    this.watch = false;
    this.away = false;
    this.ran = false;
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
    this.ids = rows.map(rowId).filter((id) => id !== undefined);
    const previous = this.newestUser;
    const newest = newestUserRow(rows);
    this.newestUser = newest ? (rowId(newest) ?? null) : null;
    if (!this.started) {
      if (rows.length > 0 && !this.hide(layout.viewport)) this.start();
      return;
    }
    if (this.newestUser !== null && this.newestUser !== previous) {
      const sent = this.key !== null && sentTurns.get(this.key) === this.newestUser;
      const edited = this.editSentAtMs !== null && performance.now() - this.editSentAtMs < EDIT_WINDOW_MS;
      if (sent || edited) {
        if (sent && this.key !== null) sentTurns.delete(this.key);
        this.editSentAtMs = null;
        // Scrolled up reading, the user stays where they are: the turn is added below.
        if (sent && this.away && !this.follow) {
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
   * Whether the thread works on its newest turn: a sent turn is watched while it does. Work that
   * starts again with no message sent here (a plan approved, a question answered, a queued
   * message taken up) is watched too unless the user scrolled away, so it is followed once it
   * reaches past the view's bottom.
   */
  setRunning(running: boolean): void {
    const resumed = running && !this.running;
    // Growth not heard of yet is handled while the watch still holds: the last of a turn comes
    // in the same update as its end, and a window that paints nothing (in the background) hears
    // of no resize at all.
    if (!running && this.running && this.watch) this.resized();
    this.running = running;
    if (running) this.ran = true;
    else if (this.ran) this.turnEnded();
    if (resumed && this.started && !this.away && !this.follow) this.watch = true;
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
    // Shown again: the browser may have reset the view's top while it was hidden.
    const shown = this.hidden;
    this.hidden = false;
    // A jump's move made in a frame callback is heard as a scroll only with the next frame, after
    // a resize in this one: the view is where the jump put it, not at the anchor from before.
    if (this.jumped && Math.abs(viewport.scrollTop - this.expected) >= 1) this.scrolled();
    const wasAtBottom = this.atBottom;
    this.updateRoom();
    const contentChanged = layout.content() !== this.contentHeight;
    const viewChanged =
      viewport.clientWidth !== this.size.width ||
      viewport.clientHeight !== this.size.height ||
      layout.padding() !== this.paddingPx;
    this.size = { width: viewport.clientWidth, height: viewport.clientHeight };
    this.paddingPx = layout.padding();
    if (this.glide !== null) {
      this.record();
      return;
    }
    const held = this.held;
    if (held && (performance.now() > held.untilMs || !held.row.isConnected)) this.held = null;
    if (this.restore) this.applyRestore();
    else if (this.held) this.keep(this.held);
    else if (((viewChanged || shown) && wasAtBottom) || (this.follow && (contentChanged || viewChanged || shown))) {
      this.toBottom();
      this.record();
      return;
    } else if (this.watch && this.overflows()) {
      // The sent turn reached past the view's bottom: from here it is followed.
      this.watch = false;
      this.follow = true;
      this.dropRoom();
      this.setRoomPx(0);
      this.toBottom();
      this.record();
      return;
    } else if (this.anchor) this.keep(this.anchor);
    this.record(false);
  }

  /**
   * A scroll event. One that didn't come from here is the user's when it comes with their input
   * (a wheel, a touch, a held pointer, a key, a focus moved in, a jump) or right after a scroll of theirs (a fling
   * going on); any other is the browser's: clamping the
   * view to a range that shrank under it (a turn re-laid out as its work starts again, say, its
   * content back before the scroll is heard), hiding or showing it. The browser's moves nothing:
   * a view that follows goes back to its bottom.
   */
  scrolled(): void {
    const viewport = this.layout?.viewport;
    if (!viewport || this.hide(viewport)) return;
    if (this.hidden) {
      this.resized();
      return;
    }
    this.jumped = false;
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
      this.record();
      return;
    }
    this.userScrollAtMs = now;
    this.interrupt();
    const distance = max - viewport.scrollTop;
    if (viewport.scrollTop < this.expected) {
      if (distance > FOLLOW_PX) {
        // Away from the bottom: nothing follows, and the room scrolled below the view goes.
        this.follow = false;
        this.watch = false;
        this.away = true;
        this.trimRoom();
      }
    } else if (distance <= FOLLOW_PX) {
      this.follow = true;
      this.watch = false;
      this.away = false;
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
   * The user's own input in the thread (a wheel, a touch, a key, a pointer pressed or let go):
   * what was underway stops, and the scrolls that come with it are theirs.
   */
  input = (event?: { type: string }): void => {
    actions += 1;
    this.inputAtMs = performance.now();
    if (event?.type === "pointerdown") this.pressed = true;
    else if (event?.type === "pointerup" || event?.type === "pointercancel") this.pressed = false;
    if (event?.type !== "pointerup" && event?.type !== "pointercancel") this.interrupt();
  };

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
    this.follow = false;
    this.watch = false;
    this.away = true;
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
    if (fold) this.watch = false;
    if (this.follow && this.roomPx === 0 && this.lastDistance <= FOLLOW_PX) return;
    const viewTop = layout.viewport.getBoundingClientRect().top;
    const top = toggle.getBoundingClientRect().top;
    this.held = { row: toggle, offset: top - viewTop, untilMs: performance.now() + HOLD_MS };
    if (fold) this.follow = false;
  }

  /**
   * The scroll button: to the bottom, following from there. While the thread works it jumps
   * there at once, without the room; otherwise it glides.
   */
  scrollToBottom = (): void => {
    const layout = this.layout;
    if (!layout) return;
    actions += 1;
    this.interrupt();
    this.follow = true;
    this.watch = false;
    this.away = false;
    if (this.running) {
      this.dropRoom();
      this.setRoomPx(0);
      this.toBottom();
      this.record();
      return;
    }
    this.glideTo(() => maxScroll(layout.viewport), easeOut);
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
      this.place(this.newestUser);
      return;
    }
    const kept = key === null ? undefined : saved.get(key);
    if (kept && !kept.bottom) {
      // The room that held the place, less what its turn grew by while away.
      if (kept.room && kept.room.id === this.newestUser) {
        this.room = kept.room.id;
        this.roomUsed = roomFor(this.visible()) - kept.room.px;
        this.roomTurn = kept.room.turn;
        this.updateRoom();
      }
      this.away = true;
      this.follow = false;
      this.restore = this.restored(kept);
    }
    if (this.restore) {
      this.applyRestore();
      this.record(false);
      return;
    }
    this.follow = true;
    this.away = false;
    this.dropRoom();
    this.setRoomPx(0);
    this.toBottom();
    this.record();
  }

  /** The kept anchor's row, or the nearest one before it that is still there. */
  private restored(kept: Saved): Anchor | null {
    if (!kept.anchor) return null;
    const rows = new Map<string, RowBox>();
    for (const row of this.layout?.rows() ?? []) {
      const id = rowId(row);
      if (id !== undefined) rows.set(id, row);
    }
    const at = kept.ids.indexOf(kept.anchor.id);
    const ids = at < 0 ? [kept.anchor.id] : kept.ids.slice(0, at + 1).toReversed();
    for (const id of ids) {
      const row = rows.get(id);
      if (row) return { row, offset: kept.anchor.offset };
    }
    return null;
  }

  /**
   * Puts the kept row back at its offset; once nothing clamps it and the row reaches the view's
   * top (a reply still loading may be shorter than the offset into it), the anchor keeps it there.
   */
  private applyRestore(): void {
    const restore = this.restore;
    const viewport = this.layout?.viewport;
    if (!restore || !restore.row.isConnected || !viewport) {
      this.restore = null;
      return;
    }
    const target = viewport.scrollTop + this.offsetOf(restore.row) - restore.offset;
    this.setTop(target);
    this.anchor = restore;
    const reached = restore.row.getBoundingClientRect().bottom > viewport.getBoundingClientRect().top;
    if (target >= 0 && target <= maxScroll(viewport) && reached) this.restore = null;
  }

  /** Gives the turn of user message `id` its room and springs to the bottom, watching it. */
  private place(id: string): void {
    const layout = this.layout;
    if (!layout) return;
    this.interrupt();
    this.follow = false;
    this.watch = true;
    this.away = false;
    this.dropRoom();
    this.room = id;
    this.ran = this.running;
    this.updateRoom();
    this.glideTo(() => maxScroll(layout.viewport), spring);
    this.record();
  }

  /** The view's height above the footer's gap. */
  private visible(): number {
    const layout = this.layout;
    return layout ? layout.viewport.clientHeight - layout.padding() : 0;
  }

  /**
   * The newest turn's room: `roomFor` the view, less what is used up. The turn's growth uses it
   * up, its shrinking gives nothing back, and at 0 it is gone.
   */
  private updateRoom(): void {
    const layout = this.layout;
    if (!layout || this.hide(layout.viewport)) return;
    let px = 0;
    if (this.room !== null) {
      const rows = layout.rows();
      const user = newestUserRow(rows);
      const last = rows.at(-1);
      if (user && last && rowId(user) === this.room) {
        const turn = last.getBoundingClientRect().bottom - user.getBoundingClientRect().top;
        if (this.roomTurn !== null && turn > this.roomTurn) this.roomUsed += turn - this.roomTurn;
        this.roomTurn = turn;
        px = Math.max(0, roomFor(this.visible()) - this.roomUsed);
      }
      if (px === 0) this.dropRoom();
    }
    this.setRoomPx(px);
  }

  /** The room below the view goes: it is used up, so what shows stays where it is. */
  private trimRoom(): void {
    const viewport = this.layout?.viewport;
    if (!viewport || this.roomPx === 0) return;
    const below = maxScroll(viewport) - viewport.scrollTop;
    if (below <= 0) return;
    this.roomUsed += Math.min(this.roomPx, below);
    this.updateRoom();
  }

  /** The room's turn ended: it is no longer watched, and the room left is only what shows. */
  private turnEnded(): void {
    this.ran = false;
    this.watch = false;
    const layout = this.layout;
    if (!layout || this.hide(layout.viewport)) return;
    this.trimRoom();
    this.record(false);
  }

  private dropRoom(): void {
    this.room = null;
    this.roomUsed = 0;
    this.roomTurn = null;
  }

  private setRoomPx(px: number): void {
    if (px === this.roomPx) return;
    this.roomPx = px;
    this.layout?.setRoom(px);
  }

  /** Whether the newest row reaches past the view's bottom, above the footer's gap. */
  private overflows(): boolean {
    const layout = this.layout;
    const last = layout?.rows().at(-1);
    if (!layout || !last) return false;
    const end = layout.viewport.getBoundingClientRect().bottom - layout.padding();
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
   * at 0. The target is read again each frame, as content changes; when it moves, the way left
   * is rescaled from where the view is, so the view never jumps or turns back (a placeholder
   * under a sent message giving way to its reply makes the turn shorter mid-spring).
   */
  private glideTo(target: () => number, curve: (ms: number) => number): void {
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
    const step = (now: number) => {
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
      const done = Math.abs(offset * left) < 0.5;
      this.setTop(goal - (done ? 0 : offset * left));
      this.glide = done ? null : requestAnimationFrame(step);
      this.record();
    };
    this.glide = requestAnimationFrame(step);
  }

  private stopGlide(): void {
    if (this.glide !== null) cancelAnimationFrame(this.glide);
    this.glide = null;
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
    this.contentHeight = layout.content();
    const rows = layout.rows();
    const viewRect = viewport.getBoundingClientRect();
    const viewTop = viewRect.top;
    const lastBottom = rows.at(-1)?.getBoundingClientRect().bottom;
    // Content in the gap above the footer is in view; only what reaches under the footer is hidden.
    this.below = lastBottom !== undefined && lastBottom > viewRect.bottom - layout.footer() + BELOW_PX;
    // The first row whose bottom is below the view's top: the rows are in order.
    let low = 0;
    let high = rows.length;
    while (low < high) {
      const middle = (low + high) >> 1;
      if ((rows[middle]?.getBoundingClientRect().bottom ?? 0) > viewTop) high = middle;
      else low = middle + 1;
    }
    // A restore still underway keeps its row and offset as the place.
    const row = this.restore ? this.restore.row : rows[Math.min(low, rows.length - 1)];
    if (this.restore) this.anchor = this.restore;
    else if (measured || this.anchor?.row !== row) {
      this.anchor = row ? { row, offset: row.getBoundingClientRect().top - viewTop } : null;
    }
    if (this.started && this.key !== null) {
      const id = row ? rowId(row) : undefined;
      // At the bottom (a sent turn's room in view counts) and not scrolled away: back at the bottom.
      const bottom = this.follow || (!this.away && this.lastDistance <= FOLLOW_PX);
      saved.set(this.key, {
        bottom,
        anchor: id !== undefined && this.anchor ? { id, offset: this.anchor.offset } : null,
        ids: this.ids,
        room:
          !bottom && this.room !== null && this.roomTurn !== null
            ? { id: this.room, px: this.roomPx, turn: this.roomTurn }
            : null,
      });
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
