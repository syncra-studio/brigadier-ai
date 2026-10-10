/*
 * The thread's scrolling, matched to ChatGPT's app. The thread lays out top-down with the
 * browser's own scroll anchoring off (WebKit has none), so content growing below the view moves
 * nothing. What does move is decided here:
 * - An anchor keeps its place on screen while anything above it changes size or loads: the row
 *   crossing the view's top, or for a moment the toggle the user just clicked.
 * - The thread follows new content (stays at its bottom) only once the user went to the bottom
 *   themselves, by scrolling there or with the scroll button; scrolling away, sending and opening
 *   a fold stop it. The floating composer changing size never moves the view.
 * - A sent message's turn glides up to its `scroll-margin-top` under the view's top, with room
 *   below it (the newest turn's minimum height) that its answer fills as it streams.
 * - Each conversation's place is kept for the app's run, by row id, and restored on return.
 * - While the thread is hidden (`display: none`, under a fullscreen side panel) nothing is measured
 *   or kept; shown again, it is put back where it was, or at the bottom if it was there.
 */

/** Within this of the bottom the thread is at its bottom: no scroll button, and it follows. */
export const AT_BOTTOM_PX = 8;
/** The glide to a sent turn or to the bottom (easing out). */
const GLIDE_MS = 300;
/** How long a clicked toggle keeps its place: the longest fold animation, with a frame to spare. */
const HOLD_MS = 350;
/** An edit's new message comes back from the daemon; one that comes later than this isn't placed. */
const EDIT_WINDOW_MS = 30_000;

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
  /** How far under the view's top a sent message's row settles. */
  turnTop: (row: RowBox) => number;
  /** The room after the rows. */
  setRoom: (px: number) => void;
};

/** A conversation's place while it is not shown. */
type Saved = {
  /** The row at the view's top and its top's offset from the view's top. */
  anchor: { id: string; offset: number } | null;
  follow: boolean;
  /** The rows' ids then, in order: an anchor that is gone falls back to the nearest one before it. */
  ids: readonly string[];
  /** The user message whose turn has room. */
  room: string | null;
};
const saved = new Map<string, Saved>();

/** Messages just sent, by conversation: their row's id, placed when it shows. */
const sentTurns = new Map<string, string>();

/**
 * The user sent the message shown as row `id` in conversation `key`: its turn is placed when
 * it shows, also in a thread not yet mounted (the first message, sent from the new-chat view).
 */
export function expectSentTurn(key: string, id: string): void {
  sentTurns.set(key, id);
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
  /** scrollTop as last set or seen here: a scroll to anywhere else is the user's. */
  private expected = 0;
  private lastDistance = 0;
  /** The view's size as last handled, and whether the view was at its bottom then. */
  private size = { width: 0, height: 0 };
  private atBottom = true;
  private contentHeight = 0;
  private anchor: Anchor | null = null;
  /** The toggle just clicked, kept where it was until its fold settles. */
  private held: (Anchor & { untilMs: number }) | null = null;
  /** The place asked for on showing: applied again as content loads, until reached or the user acts. */
  private restore: Anchor | null = null;
  /** The user message whose turn has room, and the room's height. */
  private room: string | null = null;
  private roomPx = 0;
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

  /** Whether the view is above the bottom: the scroll button shows. */
  contentBelow = (): boolean => this.lastDistance > AT_BOTTOM_PX;

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
    this.follow = true;
    this.anchor = null;
    this.held = null;
    this.restore = null;
    this.room = null;
    this.roomPx = 0;
    this.newestUser = null;
    this.lastDistance = 0;
    layout.setRoom(0);
    this.rowsChanged();
    return () => {
      this.stopGlide();
      if (this.layout === layout) this.layout = null;
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
        this.place(this.newestUser);
        return;
      }
    }
    this.resized();
  }

  /** The user sent an edit of one of their messages: its new message's turn is placed. */
  editSent = (): void => {
    this.editSentAtMs = performance.now();
  };

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
    const wasAtBottom = this.atBottom;
    this.updateRoom();
    const contentChanged = layout.content() !== this.contentHeight;
    const viewChanged =
      viewport.clientWidth !== this.size.width || viewport.clientHeight !== this.size.height;
    this.size = { width: viewport.clientWidth, height: viewport.clientHeight };
    if (this.glide !== null) {
      this.record();
      return;
    }
    const held = this.held;
    if (held && (performance.now() > held.untilMs || !held.row.isConnected)) this.held = null;
    if (this.restore) this.applyRestore();
    else if (this.held) this.keep(this.held);
    else if (
      ((viewChanged || shown) && wasAtBottom) ||
      (this.follow && (contentChanged || shown))
    ) {
      this.toBottom();
      this.record();
      return;
    } else if (this.anchor) this.keep(this.anchor);
    this.record(false);
  }

  /**
   * A scroll event. One that didn't come from here is the user's, except the browser clamping
   * the view to a range that shrank under it, or hiding or showing it.
   */
  scrolled(): void {
    const viewport = this.layout?.viewport;
    if (!viewport || this.hide(viewport)) return;
    if (this.hidden) {
      this.resized();
      return;
    }
    const max = maxScroll(viewport);
    const clamped = this.expected > max && Math.abs(viewport.scrollTop - max) < 1;
    if (clamped || Math.abs(viewport.scrollTop - this.expected) < 1) {
      // Clamped, the view's top moved: its anchor is where it is now.
      this.record(clamped);
      return;
    }
    this.interrupt();
    this.follow = max - viewport.scrollTop <= AT_BOTTOM_PX;
    this.record();
  }

  /** The user scrolls, presses, clicks or types in the thread: what was underway stops. */
  interrupt = (): void => {
    this.stopGlide();
    this.restore = null;
    this.held = null;
  };

  /**
   * A click in the thread: a button or toggle keeps its place while what it opens or closes
   * settles, and opening or closing a fold stops following.
   */
  clicked(target: Element): void {
    const viewport = this.layout?.viewport;
    const toggle = target.closest<HTMLElement>("button, summary, [role=button], [aria-expanded]");
    if (!viewport || !toggle) return;
    const offset = toggle.getBoundingClientRect().top - viewport.getBoundingClientRect().top;
    this.held = { row: toggle, offset, untilMs: performance.now() + HOLD_MS };
    if (toggle.matches("summary, [aria-expanded]")) this.follow = false;
  }

  /** The scroll button: glides to the bottom and follows from there. */
  scrollToBottom = (): void => {
    const layout = this.layout;
    if (!layout) return;
    this.interrupt();
    this.follow = true;
    this.glideTo(() => maxScroll(layout.viewport));
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
    const key = this.key;
    if (key !== null && this.newestUser !== null && sentTurns.get(key) === this.newestUser) {
      sentTurns.delete(key);
      this.place(this.newestUser);
      return;
    }
    const kept = key === null ? undefined : saved.get(key);
    if (kept) {
      this.room = kept.room !== null && kept.room === this.newestUser ? kept.room : null;
      this.updateRoom();
      this.follow = kept.follow;
      this.restore = kept.follow ? null : this.restored(kept);
    }
    if (this.restore) {
      this.applyRestore();
      this.record(false);
      return;
    }
    this.follow = true;
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

  /** Gives the turn of user message `id` its room and glides it up under the view's top. */
  private place(id: string): void {
    const layout = this.layout;
    if (!layout) return;
    this.interrupt();
    this.follow = false;
    this.room = id;
    this.updateRoom();
    const row = layout.rows().find((candidate) => rowId(candidate) === id);
    const viewport = layout.viewport;
    if (row) {
      this.glideTo(() =>
        row.isConnected ? viewport.scrollTop + this.offsetOf(row) - layout.turnTop(row) : viewport.scrollTop,
      );
    }
    this.record();
  }

  /** The newest turn's room: down to the composer's gap, with the turn's top at its place. */
  private updateRoom(): void {
    const layout = this.layout;
    if (!layout || this.hide(layout.viewport)) return;
    let px = 0;
    if (this.room !== null) {
      const rows = layout.rows();
      const user = newestUserRow(rows);
      const last = rows.at(-1);
      if (!user || !last || rowId(user) !== this.room) this.room = null;
      else {
        const turn = last.getBoundingClientRect().bottom - user.getBoundingClientRect().top;
        px = Math.max(0, layout.viewport.clientHeight - layout.turnTop(user) - layout.padding() - turn);
      }
    }
    if (px !== this.roomPx) {
      this.roomPx = px;
      layout.setRoom(px);
    }
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

  private glideTo(target: () => number): void {
    const viewport = this.layout?.viewport;
    if (!viewport) return;
    this.stopGlide();
    if (reducedMotion()) {
      this.setTop(target());
      return;
    }
    const from = viewport.scrollTop;
    const begun = performance.now();
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
      const progress = Math.min(1, Math.max(0, (now - begun) / GLIDE_MS));
      const eased = 1 - (1 - progress) ** 3;
      this.setTop(from + (target() - from) * eased);
      this.glide = progress < 1 ? requestAnimationFrame(step) : null;
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
      this.atBottom = this.lastDistance <= AT_BOTTOM_PX;
    }
    this.contentHeight = layout.content();
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
    // A restore still underway keeps its row and offset as the place.
    const row = this.restore ? this.restore.row : rows[Math.min(low, rows.length - 1)];
    if (this.restore) this.anchor = this.restore;
    else if (measured || this.anchor?.row !== row) {
      this.anchor = row ? { row, offset: row.getBoundingClientRect().top - viewTop } : null;
    }
    if (this.started && this.key !== null) {
      const id = row ? rowId(row) : undefined;
      saved.set(this.key, {
        anchor: id !== undefined && this.anchor ? { id, offset: this.anchor.offset } : null,
        follow: this.follow,
        ids: this.ids,
        room: this.room,
      });
    }
    for (const listener of this.listeners) listener();
  }
}

function maxScroll(viewport: ScrollBox): number {
  return Math.max(0, viewport.scrollHeight - viewport.clientHeight);
}

function newestUserRow(rows: readonly RowBox[]): RowBox | undefined {
  return rows.findLast((row) => row.dataset["role"] === "user");
}
