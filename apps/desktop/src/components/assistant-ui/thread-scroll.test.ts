import assert from "node:assert/strict";
import { beforeEach, test } from "node:test";

import {
  AT_BOTTOM_PX,
  expectSentTurn,
  type RowBox,
  ThreadScroller,
} from "@/components/assistant-ui/thread-scroll";

/*
 * A thread laid out by hand: the view's top at 100px, 32px above the rows, 24px between them,
 * the room after them, then the pad (the footer's height and the 84px gap). scrollTop clamps
 * to the range like a browser's.
 */
const VIEW_TOP = 100;
const TOP_PAD = 32;
const GAP = 24;
const COMPOSER_GAP = 84;
const TURN_TOP = 171;

type Row = RowBox & { height: number; dataset: DOMStringMap; isConnected: boolean };

class Fake {
  clientHeight = 976;
  clientWidth = 800;
  footer = 120;
  room = 0;
  rows: Row[] = [];
  private top = 0;

  get padding(): number {
    return this.footer + COMPOSER_GAP;
  }

  /** From the column's top to the end of the room. */
  get content(): number {
    const rows = this.rows.reduce((sum, row, index) => sum + row.height + (index > 0 ? GAP : 0), 0);
    return TOP_PAD + rows + this.room;
  }

  get scrollHeight(): number {
    if (this.clientHeight === 0) return 0;
    return Math.max(this.clientHeight, this.content + this.padding);
  }

  get max(): number {
    return Math.max(0, this.scrollHeight - this.clientHeight);
  }

  get scrollTop(): number {
    return Math.min(this.top, this.max);
  }

  set scrollTop(value: number) {
    this.top = Math.max(0, Math.min(value, this.max));
  }

  /** The row's top in the content (from the column's top). */
  rowTop(row: Row): number {
    let top = TOP_PAD;
    for (const other of this.rows) {
      if (other === row) return top;
      top += other.height + GAP;
    }
    throw new Error("not a row");
  }

  row(id: string, height: number, role: "user" | "assistant" = "assistant"): Row {
    const row: Row = {
      height,
      dataset: { messageId: id, role },
      isConnected: true,
      getBoundingClientRect: () => {
        if (this.clientHeight === 0) return { top: 0, bottom: 0 };
        const top = VIEW_TOP + this.rowTop(row) - this.scrollTop;
        return { top, bottom: top + row.height };
      },
    };
    return row;
  }

  getBoundingClientRect() {
    if (this.clientHeight === 0) return { top: 0, bottom: 0 };
    return { top: VIEW_TOP, bottom: VIEW_TOP + this.clientHeight };
  }

  /** Distance from the view's top of a row's top. */
  offset(row: Row): number {
    return row.getBoundingClientRect().top - VIEW_TOP;
  }

  layout() {
    return {
      viewport: this,
      rows: () => this.rows,
      content: () => this.content,
      padding: () => this.padding,
      turnTop: () => TURN_TOP,
      setRoom: (px: number) => {
        this.room = px;
      },
    };
  }
}

/** A conversation of `turns` user and assistant rows. */
function conversation(fake: Fake, turns: number, prefix = ""): void {
  for (let index = 0; index < turns; index++) {
    fake.rows.push(fake.row(`${prefix}u${index}`, 60, "user"), fake.row(`${prefix}a${index}`, 400));
  }
}

/** A toggle inside `row`, `at` px below its top. */
function toggle(row: Row, at: number, expanded = true): HTMLElement {
  const element = {
    dataset: {},
    get isConnected() {
      return row.isConnected;
    },
    getBoundingClientRect() {
      const top = row.getBoundingClientRect().top + at;
      return { top, bottom: top + 20 };
    },
    closest: () => element,
    matches: (selector: string) => expanded && selector.includes("aria-expanded"),
  };
  return element as unknown as HTMLElement;
}

let frames: FrameRequestCallback[] = [];
let reduced = true;
let keys = 0;
const key = () => `conversation-${keys++}`;

beforeEach(() => {
  frames = [];
  reduced = true;
  Object.assign(globalThis, {
    matchMedia: () => ({ matches: reduced }),
    requestAnimationFrame: (callback: FrameRequestCallback) => frames.push(callback),
    cancelAnimationFrame: () => {},
  });
});

/** Runs the queued frames, `ms` apart, until none are left. */
function flushFrames(scroller: ThreadScroller, ms = 50): void {
  let now = performance.now();
  for (let round = 0; round < 100 && frames.length > 0; round++) {
    now += ms;
    const queued = frames;
    frames = [];
    for (const frame of queued) frame(now);
    scroller.scrolled();
  }
  assert.equal(frames.length, 0);
}

function open(fake: Fake, id: string | null = key()): ThreadScroller {
  const scroller = new ThreadScroller();
  scroller.attach(fake.layout(), id);
  return scroller;
}

/** The user scrolls to `top`. */
function userScroll(fake: Fake, scroller: ThreadScroller, top: number): void {
  scroller.interrupt();
  fake.scrollTop = top;
  scroller.scrolled();
}

/**
 * The thread hidden (`display: none`, under a fullscreen panel): no size, and the browser resets
 * its top. The returned call shows it again, the reset top's scroll event before its resize.
 */
function hide(fake: Fake, scroller: ThreadScroller): () => void {
  const { clientHeight, clientWidth } = fake;
  fake.clientHeight = 0;
  fake.clientWidth = 0;
  fake.scrollTop = 0;
  scroller.scrolled();
  scroller.resized();
  return () => {
    fake.clientHeight = clientHeight;
    fake.clientWidth = clientWidth;
    scroller.scrolled();
    scroller.resized();
  };
}

test("a first open lands at the bottom and stays there while content loads late", () => {
  const fake = new Fake();
  conversation(fake, 10);
  const scroller = open(fake);
  assert.equal(fake.scrollTop, fake.max);
  assert.equal(scroller.following, true);
  assert.equal(scroller.contentBelow(), false);
  fake.rows[1]!.height += 300;
  fake.rows.at(-1)!.height += 200;
  scroller.resized();
  assert.equal(fake.scrollTop, fake.max);
});

test("at the bottom the last content ends the composer gap above the composer's top", () => {
  const fake = new Fake();
  conversation(fake, 10);
  open(fake);
  const last = fake.rows.at(-1)!;
  const composerTop = VIEW_TOP + fake.clientHeight - fake.footer;
  assert.equal(composerTop - last.getBoundingClientRect().bottom, COMPOSER_GAP);
});

test("scrolled up, content changing above the view keeps what shows still; below moves nothing", () => {
  const fake = new Fake();
  conversation(fake, 10);
  const scroller = open(fake);
  userScroll(fake, scroller, 2000);
  assert.equal(scroller.following, false);
  assert.equal(scroller.contentBelow(), true);
  const shown = fake.rows.find((row) => row.getBoundingClientRect().bottom > VIEW_TOP)!;
  const before = fake.offset(shown);
  fake.rows[0]!.height += 250;
  scroller.resized();
  assert.equal(fake.offset(shown), before);
  const top = fake.scrollTop;
  fake.rows.at(-1)!.height += 500;
  scroller.resized();
  assert.equal(fake.scrollTop, top);
});

test("following starts when the user scrolls to the bottom and stops when they scroll up", () => {
  const fake = new Fake();
  conversation(fake, 10);
  const scroller = open(fake);
  userScroll(fake, scroller, 1000);
  fake.rows.at(-1)!.height += 300;
  scroller.resized();
  assert.ok(fake.max - fake.scrollTop > AT_BOTTOM_PX);
  userScroll(fake, scroller, fake.max - AT_BOTTOM_PX + 2);
  assert.equal(scroller.following, true);
  fake.rows.at(-1)!.height += 300;
  scroller.resized();
  assert.equal(fake.scrollTop, fake.max);
  userScroll(fake, scroller, fake.max - 200);
  assert.equal(scroller.following, false);
});

test("the button shows past the bottom's threshold, and glides to the bottom to follow", () => {
  const fake = new Fake();
  conversation(fake, 10);
  const scroller = open(fake);
  userScroll(fake, scroller, fake.max - AT_BOTTOM_PX);
  assert.equal(scroller.contentBelow(), false);
  userScroll(fake, scroller, fake.max - AT_BOTTOM_PX - 1);
  assert.equal(scroller.contentBelow(), true);
  userScroll(fake, scroller, 500);
  reduced = false;
  scroller.scrollToBottom();
  // Content grows during the glide: it ends at the new bottom.
  fake.rows.at(-1)!.height += 100;
  flushFrames(scroller);
  assert.equal(fake.scrollTop, fake.max);
  assert.equal(scroller.following, true);
});

test("the composer growing or shrinking never moves the view, even while following", () => {
  const fake = new Fake();
  conversation(fake, 10);
  const scroller = open(fake);
  assert.equal(scroller.following, true);
  const top = fake.scrollTop;
  fake.footer = 320;
  scroller.resized();
  assert.equal(fake.scrollTop, top);
  assert.equal(scroller.contentBelow(), true);
  fake.footer = 120;
  // The range shrinks back: the view clamps to the bottom again.
  scroller.resized();
  scroller.scrolled();
  assert.equal(fake.scrollTop, fake.max);
  assert.equal(scroller.following, true);
  // Scrolled up, too.
  userScroll(fake, scroller, 1500);
  fake.footer = 320;
  scroller.resized();
  assert.equal(fake.scrollTop, 1500);
});

test("a clicked toggle stays in place while its fold opens, mid-scroll and at the bottom", () => {
  const fake = new Fake();
  conversation(fake, 10);
  const scroller = open(fake);
  userScroll(fake, scroller, fake.rowTop(fake.rows[9]!) - 300);
  const row = fake.rows[9]!;
  const header = toggle(row, 30);
  const before = header.getBoundingClientRect().top;
  scroller.clicked(header);
  row.height += 400;
  // A row above changes at the same time (its own content loading).
  fake.rows[2]!.height += 50;
  scroller.resized();
  assert.equal(header.getBoundingClientRect().top, before);

  // At the bottom, following: opening the last fold leaves the view above the new bottom.
  const end = new Fake();
  conversation(end, 10);
  const following = open(end);
  const last = end.rows.at(-1)!;
  const lastHeader = toggle(last, 30);
  const at = lastHeader.getBoundingClientRect().top;
  following.clicked(lastHeader);
  assert.equal(following.following, false);
  last.height += 400;
  following.resized();
  assert.equal(lastHeader.getBoundingClientRect().top, at);
  assert.equal(following.contentBelow(), true);
  // Closing it again shrinks the range back to the bottom.
  following.clicked(lastHeader);
  last.height -= 400;
  following.resized();
  following.scrolled();
  assert.equal(end.scrollTop, end.max);
});

test("a sent message settles at its place under the view's top, with room for its answer", () => {
  const fake = new Fake();
  conversation(fake, 6);
  const id = key();
  const scroller = open(fake, id);
  expectSentTurn(id, "sent");
  const user = fake.row("sent", 60, "user");
  fake.rows.push(user);
  reduced = false;
  scroller.rowsChanged();
  flushFrames(scroller);
  assert.equal(fake.offset(user), TURN_TOP);
  assert.equal(scroller.following, false);
  // The room reaches down to the composer's gap: the view is at the bottom.
  assert.equal(fake.scrollTop, fake.max);
  assert.equal(fake.room, fake.clientHeight - TURN_TOP - fake.padding - 60);
  // The answer streams into the room: nothing moves.
  const answer = fake.row("answer", 40);
  fake.rows.push(answer);
  scroller.rowsChanged();
  for (let grow = 0; grow < 30; grow++) {
    answer.height += 40;
    scroller.resized();
    assert.equal(fake.offset(user), TURN_TOP);
  }
  // Past the room it goes on under the composer, unfollowed.
  assert.equal(fake.room, 0);
  assert.equal(scroller.contentBelow(), true);
  // Going to the bottom follows it from there.
  userScroll(fake, scroller, fake.max);
  answer.height += 300;
  scroller.resized();
  assert.equal(fake.scrollTop, fake.max);
});

test("in an empty conversation a sent message sits at the top padding", () => {
  const fake = new Fake();
  const id = key();
  const scroller = open(fake, id);
  expectSentTurn(id, "first");
  const user = fake.row("first", 60, "user");
  fake.rows.push(user);
  scroller.rowsChanged();
  assert.equal(fake.scrollTop, 0);
  assert.equal(fake.offset(user), TOP_PAD);
  assert.equal(scroller.following, false);
});

test("the first message sent from the new-chat view is placed in the conversation it starts", () => {
  const id = key();
  expectSentTurn(id, "draft");
  const fake = new Fake();
  conversation(fake, 3);
  fake.rows.push(fake.row("draft", 60, "user"));
  const scroller = open(fake, id);
  assert.equal(scroller.following, false);
  assert.ok(fake.room > 0);
  assert.equal(fake.offset(fake.rows.at(-1)!), TURN_TOP);
});

test("an unseen conversation with a running turn opens at the bottom", () => {
  const fake = new Fake();
  conversation(fake, 1);
  const scroller = open(fake);
  assert.equal(scroller.following, true);
  assert.equal(fake.room, 0);
  assert.equal(fake.scrollTop, fake.max);
});

test("a user row not sent here doesn't move the view", () => {
  const fake = new Fake();
  conversation(fake, 6);
  const scroller = open(fake);
  userScroll(fake, scroller, 1000);
  fake.rows.push(fake.row("elsewhere", 60, "user"));
  scroller.rowsChanged();
  assert.equal(fake.scrollTop, 1000);
  assert.equal(fake.room, 0);
});

test("a sent edit's new message is placed", () => {
  const fake = new Fake();
  conversation(fake, 6);
  const scroller = open(fake);
  scroller.editSent();
  const edited = fake.row("edited", 60, "user");
  fake.rows.splice(-2, 2, edited);
  scroller.rowsChanged();
  assert.equal(fake.offset(edited), TURN_TOP);
});

test("a conversation reopens exactly where it was left, by row id", () => {
  const id = key();
  const first = new Fake();
  conversation(first, 10);
  const scroller = open(first, id);
  userScroll(first, scroller, 2345);
  const top = first.scrollTop;

  const again = new Fake();
  conversation(again, 10);
  open(again, id);
  assert.equal(again.scrollTop, top);

  // Earlier rows loaded above it, and later ones below, while it was away: the same row shows.
  const grown = new Fake();
  grown.rows.push(grown.row("older-u", 60, "user"), grown.row("older-a", 400));
  conversation(grown, 10);
  conversation(grown, 2, "new-");
  const reopened = open(grown, id);
  const anchor = first.rows.find((row) => row.getBoundingClientRect().bottom > VIEW_TOP)!;
  const same = grown.rows.find((row) => row.dataset["messageId"] === anchor.dataset["messageId"])!;
  assert.equal(grown.offset(same), first.offset(anchor));
  assert.equal(reopened.following, false);
});

test("a reopened conversation keeps asking for its place while its content loads", () => {
  const id = key();
  const first = new Fake();
  conversation(first, 10);
  const scroller = open(first, id);
  userScroll(first, scroller, 3000);
  const anchor = first.rows.find((row) => row.getBoundingClientRect().bottom > VIEW_TOP)!;
  const offset = first.offset(anchor);

  // It shows collapsed (its replies still loading): the place can't be reached yet.
  const loading = new Fake();
  conversation(loading, 10);
  for (const row of loading.rows) row.height = 30;
  const reopened = open(loading, id);
  const same = loading.rows.find((row) => row.dataset["messageId"] === anchor.dataset["messageId"])!;
  assert.notEqual(loading.offset(same), offset);
  for (const row of loading.rows) if (row.dataset["role"] === "assistant") row.height = 400;
  reopened.resized();
  assert.equal(loading.offset(same), offset);
  assert.equal(reopened.following, false);

  // A scroll by the user ends the asking.
  const third = new Fake();
  conversation(third, 10);
  for (const row of third.rows) row.height = 30;
  const cancelled = open(third, id);
  userScroll(third, cancelled, 10);
  for (const row of third.rows) if (row.dataset["role"] === "assistant") row.height = 400;
  cancelled.resized();
  assert.equal(third.scrollTop, 10);
});

test("a reopened place deep in a reply that loads short waits for the reply to reach it", () => {
  const id = key();
  const first = new Fake();
  conversation(first, 4);
  first.rows.push(first.row("long-u", 60, "user"), first.row("long-a", 1500));
  conversation(first, 4, "after-");
  const scroller = open(first, id);
  const long = first.rows.find((row) => row.dataset["messageId"] === "long-a")!;
  userScroll(first, scroller, first.rowTop(long) + 700);
  const offset = first.offset(long);

  // The reply first shows 100px tall: the place is in range, but past the reply.
  const loading = new Fake();
  conversation(loading, 4);
  loading.rows.push(loading.row("long-u", 60, "user"), loading.row("long-a", 100));
  conversation(loading, 4, "after-");
  const reopened = open(loading, id);
  const same = loading.rows.find((row) => row.dataset["messageId"] === "long-a")!;
  same.height = 1500;
  reopened.resized();
  assert.equal(loading.offset(same), offset);
});

test("the browser clamping the view to a shrunk range is not a scroll by the user", () => {
  const fake = new Fake();
  conversation(fake, 10);
  const scroller = open(fake);
  userScroll(fake, scroller, fake.max - 50);
  assert.equal(scroller.following, false);
  // A fold below closes: the range shrinks by 100px and the browser clamps the view.
  fake.rows.at(-1)!.height -= 100;
  scroller.scrolled();
  scroller.resized();
  assert.equal(scroller.following, false);
  // New content below then doesn't move the view.
  const top = fake.scrollTop;
  fake.rows.at(-1)!.height += 300;
  scroller.resized();
  assert.equal(fake.scrollTop, top);
});

test("a kept row that is gone falls back to the nearest one before it, or the bottom", () => {
  const id = key();
  const first = new Fake();
  conversation(first, 10);
  const scroller = open(first, id);
  userScroll(first, scroller, first.rowTop(first.rows[12]!) + 10);
  const offset = first.offset(first.rows[12]!);

  const fewer = new Fake();
  conversation(fewer, 10);
  fewer.rows.splice(12, 1);
  open(fewer, id);
  assert.equal(fewer.offset(fewer.rows[11]!), offset);

  const other = key();
  const elsewhere = new Fake();
  conversation(elsewhere, 10);
  const left = open(elsewhere, other);
  userScroll(elsewhere, left, 2000);
  const none = new Fake();
  conversation(none, 10, "replaced-");
  const reopened = open(none, other);
  assert.equal(none.scrollTop, none.max);
  assert.equal(reopened.following, true);
});

test("a conversation left at the bottom reopens at the bottom, its room kept", () => {
  const id = key();
  const first = new Fake();
  conversation(first, 6);
  const scroller = open(first, id);
  expectSentTurn(id, "sent");
  first.rows.push(first.row("sent", 60, "user"), first.row("answer", 120));
  scroller.rowsChanged();
  const top = first.scrollTop;
  const room = first.room;

  const again = new Fake();
  conversation(again, 6);
  again.rows.push(again.row("sent", 60, "user"), again.row("answer", 120));
  open(again, id);
  assert.equal(again.room, room);
  assert.equal(again.scrollTop, top);
});

test("at the bottom, a resize of the view keeps it at the bottom", () => {
  const fake = new Fake();
  conversation(fake, 10);
  const id = key();
  const scroller = open(fake, id);
  // Not following (a sent turn), yet at the bottom.
  expectSentTurn(id, "sent");
  fake.rows.push(fake.row("sent", 60, "user"), fake.row("answer", 900));
  scroller.rowsChanged();
  userScroll(fake, scroller, fake.max);
  scroller.clicked(toggle(fake.rows.at(-1)!, 10));
  scroller.interrupt();
  assert.equal(scroller.following, false);
  fake.clientHeight = 700;
  scroller.resized();
  assert.equal(fake.scrollTop, fake.max);
  fake.clientHeight = 1100;
  fake.clientWidth = 600;
  for (const row of fake.rows) row.height += 20;
  scroller.resized();
  assert.equal(fake.scrollTop, fake.max);
});

test("hidden mid-scroll, then shown, the thread is back where it was", () => {
  const fake = new Fake();
  conversation(fake, 10);
  const scroller = open(fake);
  userScroll(fake, scroller, 2000);
  const shown = fake.rows.find((row) => row.getBoundingClientRect().bottom > VIEW_TOP)!;
  const offset = fake.offset(shown);
  const show = hide(fake, scroller);
  assert.equal(scroller.contentBelow(), true);
  show();
  assert.equal(fake.scrollTop, 2000);
  assert.equal(fake.offset(shown), offset);
  assert.equal(scroller.following, false);
  assert.equal(scroller.contentBelow(), true);
});

test("hidden at the bottom, then shown, the thread stays at the bottom", () => {
  const fake = new Fake();
  conversation(fake, 10);
  const id = key();
  const scroller = open(fake, id);
  // Not following (a sent turn), yet at the bottom.
  expectSentTurn(id, "sent");
  fake.rows.push(fake.row("sent", 60, "user"), fake.row("answer", 900));
  scroller.rowsChanged();
  userScroll(fake, scroller, fake.max);
  scroller.clicked(toggle(fake.rows.at(-1)!, 10));
  scroller.interrupt();
  assert.equal(scroller.following, false);
  const show = hide(fake, scroller);
  fake.clientWidth = 600;
  show();
  assert.equal(fake.scrollTop, fake.max);
  assert.ok(fake.max > 0);
  assert.equal(scroller.following, false);
});

test("content added while hidden is followed once shown", () => {
  const fake = new Fake();
  conversation(fake, 10);
  const scroller = open(fake);
  assert.equal(scroller.following, true);
  const show = hide(fake, scroller);
  fake.rows.push(fake.row("later-u", 60, "user"), fake.row("later-a", 700));
  scroller.rowsChanged();
  fake.rows.at(-1)!.height += 300;
  scroller.resized();
  show();
  assert.equal(fake.scrollTop, fake.max);
  assert.equal(scroller.following, true);
  assert.equal(scroller.contentBelow(), false);
});
