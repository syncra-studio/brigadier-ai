import assert from "node:assert/strict";
import { beforeEach, test } from "node:test";

import {
  BELOW_PX,
  expectSentTurn,
  FOLLOW_PX,
  roomFor,
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

  /** The view's height above the composer's gap. */
  get visible(): number {
    return this.clientHeight - this.padding;
  }

  /** How far down the view a sent turn's end settles: above the room it is given. */
  get turnLine(): number {
    return this.visible - roomFor(this.visible);
  }

  /** Distance from the view's top of a row's bottom. */
  end(row: Row): number {
    return row.getBoundingClientRect().bottom - VIEW_TOP;
  }

  layout() {
    return {
      viewport: this,
      rows: () => this.rows,
      content: () => this.content,
      padding: () => this.padding,
      footer: () => this.footer,
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

function open(fake: Fake, id: string | null = key(), running = false): ThreadScroller {
  const scroller = new ThreadScroller();
  scroller.setRunning(running);
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
  assert.ok(fake.max - fake.scrollTop > FOLLOW_PX);
  userScroll(fake, scroller, fake.max - FOLLOW_PX);
  assert.equal(scroller.following, true);
  fake.rows.at(-1)!.height += 300;
  scroller.resized();
  assert.equal(fake.scrollTop, fake.max);
  // A nudge up within the threshold keeps following.
  userScroll(fake, scroller, fake.max - FOLLOW_PX);
  assert.equal(scroller.following, true);
  userScroll(fake, scroller, fake.max - FOLLOW_PX - 1);
  assert.equal(scroller.following, false);
  // Scrolling down short of the threshold doesn't follow yet.
  userScroll(fake, scroller, fake.max - FOLLOW_PX - 1 + 0.5);
  assert.equal(scroller.following, false);
});

test("the button's glide eases out over 260ms; while the thread works it jumps there without the room", () => {
  const fake = new Fake();
  conversation(fake, 10);
  const scroller = open(fake);
  userScroll(fake, scroller, 1000);
  const from = fake.scrollTop;
  reduced = false;
  scroller.scrollToBottom();
  const tops: number[] = [];
  let now = performance.now();
  frames.shift()!(now);
  while (frames.length > 0) {
    now += 20;
    frames.shift()!(now);
    tops.push(fake.scrollTop);
  }
  // 13 frames of 20ms: done at 260ms, a third of the way left at 60ms (1 - 60/260)³.
  assert.equal(tops.length, 13);
  assert.ok(Math.abs((fake.max - tops[2]!) / (fake.max - from) - (1 - 60 / 260) ** 3) < 0.01);
  assert.ok(tops.every((top, index) => index === 0 || top >= tops[index - 1]!));
  assert.equal(fake.scrollTop, fake.max);

  // Running: a sent turn's room goes, and the view is at the bottom at once.
  const working = new Fake();
  conversation(working, 6);
  const id = key();
  const sending = open(working, id, true);
  expectSentTurn(id, "sent");
  working.rows.push(working.row("sent", 60, "user"), working.row("answer", 40));
  sending.rowsChanged();
  userScroll(working, sending, 600);
  // The spring to the sent turn, cancelled by the scroll.
  frames = [];
  sending.scrollToBottom();
  assert.equal(frames.length, 0);
  assert.equal(working.room, 0);
  assert.equal(working.scrollTop, working.max);
  assert.equal(sending.following, true);
});

test("the button shows once the last row reaches under the composer, and glides to the bottom to follow", () => {
  const fake = new Fake();
  conversation(fake, 10);
  const scroller = open(fake);
  // Scrolled up through the gap above the composer, the last row is still in view.
  userScroll(fake, scroller, fake.max - COMPOSER_GAP - BELOW_PX);
  assert.equal(scroller.contentBelow(), false);
  userScroll(fake, scroller, fake.max - COMPOSER_GAP - BELOW_PX - 1);
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

test("the composer growing or shrinking keeps a following view at its bottom, and a scrolled one still", () => {
  const fake = new Fake();
  conversation(fake, 10);
  const scroller = open(fake);
  assert.equal(scroller.following, true);
  const last = fake.rows.at(-1)!;
  const gap = () => VIEW_TOP + fake.clientHeight - fake.footer - last.getBoundingClientRect().bottom;
  fake.footer = 320;
  scroller.resized();
  assert.equal(fake.scrollTop, fake.max);
  assert.equal(gap(), COMPOSER_GAP);
  assert.equal(scroller.contentBelow(), false);
  fake.footer = 120;
  scroller.resized();
  scroller.scrolled();
  assert.equal(fake.scrollTop, fake.max);
  assert.equal(gap(), COMPOSER_GAP);
  assert.equal(scroller.following, true);
  // Scrolled up, what shows stays still.
  userScroll(fake, scroller, 1500);
  fake.footer = 320;
  scroller.resized();
  assert.equal(fake.scrollTop, 1500);
});

test("a clicked toggle stays in place while its fold opens mid-scroll; at the bottom, following, the bottom stays", () => {
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

  // At the bottom, following: opening the last fold keeps the bottom where it is, so what it
  // opens shows above the composer and the content above moves up.
  const end = new Fake();
  conversation(end, 10);
  const following = open(end);
  const last = end.rows.at(-1)!;
  const lastHeader = toggle(last, 30);
  const at = lastHeader.getBoundingClientRect().top;
  following.clicked(lastHeader);
  last.height += 400;
  following.resized();
  assert.equal(following.following, true);
  assert.equal(end.scrollTop, end.max);
  assert.equal(lastHeader.getBoundingClientRect().top, at - 400);
  assert.equal(following.contentBelow(), false);
  // Closing it again: still the bottom, the content above moving down.
  following.clicked(lastHeader);
  last.height -= 400;
  following.resized();
  following.scrolled();
  assert.equal(end.scrollTop, end.max);
  assert.equal(lastHeader.getBoundingClientRect().top, at);
  assert.equal(following.following, true);
});

test("a sent turn springs up to end a third of the way down, with room its answer fills, then is followed", () => {
  const fake = new Fake();
  conversation(fake, 6);
  const id = key();
  const scroller = open(fake, id, true);
  expectSentTurn(id, "sent");
  const user = fake.row("sent", 60, "user");
  fake.rows.push(user);
  reduced = false;
  scroller.rowsChanged();
  // The spring: critically damped, no overshoot, about half the way at 50ms, settled within a
  // pixel by about half a second.
  const from = fake.scrollTop;
  const left: number[] = [];
  let now = performance.now();
  frames.shift()!(now);
  while (frames.length > 0) {
    now += 10;
    frames.shift()!(now);
    left.push((fake.max - fake.scrollTop) / (fake.max - from));
  }
  assert.ok(left.every((share, index) => share >= 0 && (index === 0 || share <= left[index - 1]!)));
  assert.ok(Math.abs(left[4]! - (1 + 0.875) * Math.exp(-0.875)) < 0.01, `at 50ms: ${left[4]}`);
  assert.ok(left.length <= 60, `settled after ${left.length * 10}ms`);
  assert.ok(left[49]! * (fake.max - from) < 1, `at 500ms: ${left[49]! * (fake.max - from)}px left`);
  // Its end sits a third of the view down; the room reaches down to the composer's gap.
  assert.equal(fake.end(user), fake.turnLine);
  assert.ok(Math.abs(fake.turnLine - Math.max(fake.visible / 3, 240)) <= 0.5);
  assert.equal(fake.room, roomFor(fake.visible));
  assert.equal(fake.scrollTop, fake.max);
  assert.equal(scroller.following, false);
  // The answer streams into the room: nothing moves.
  const answer = fake.row("answer", 40);
  fake.rows.push(answer);
  scroller.rowsChanged();
  const top = fake.offset(user);
  while (fake.room > 0) {
    answer.height += 40;
    scroller.resized();
    if (fake.room > 0) assert.equal(fake.offset(user), top);
  }
  // Past the view's bottom it is followed: its end stays at the composer's gap.
  for (let grow = 0; grow < 10; grow++) {
    answer.height += 40;
    scroller.resized();
    assert.equal(fake.scrollTop, fake.max);
  }
  assert.equal(scroller.following, true);
  assert.equal(scroller.contentBelow(), false);
});

test("a placeholder under a sent message giving way mid-spring doesn't turn the spring back", () => {
  const fake = new Fake();
  conversation(fake, 6);
  const id = key();
  const scroller = open(fake, id, true);
  expectSentTurn(id, "sent");
  const user = fake.row("sent", 60, "user");
  fake.rows.push(user, fake.row("placeholder", 90));
  reduced = false;
  scroller.rowsChanged();
  const tops: number[] = [];
  let now = performance.now();
  frames.shift()!(now);
  for (let frame = 0; frames.length > 0; frame++) {
    now += 16;
    if (frame === 5) {
      fake.rows.pop();
      scroller.rowsChanged();
    }
    frames.shift()!(now);
    tops.push(fake.scrollTop);
  }
  assert.ok(tops.every((top, index) => index === 0 || top >= tops[index - 1]!), tops.join(", "));
  assert.equal(fake.end(user), fake.turnLine);
  // Once settled, the room only shrinks again.
  const room = fake.room;
  const answer = fake.row("answer", 40);
  fake.rows.push(answer);
  scroller.rowsChanged();
  fake.rows.pop();
  scroller.rowsChanged();
  assert.equal(fake.room, room - 40 - GAP);
});

test("scrolled away from a sent turn, the room below the view goes and the turn is not followed", () => {
  const fake = new Fake();
  conversation(fake, 6);
  const id = key();
  const scroller = open(fake, id, true);
  expectSentTurn(id, "sent");
  const user = fake.row("sent", 60, "user");
  const answer = fake.row("answer", 40);
  fake.rows.push(user, answer);
  scroller.rowsChanged();
  const room = fake.room;
  // 100px up: the room under the view goes, so the view is at the new bottom without moving.
  const top = fake.scrollTop - 100;
  userScroll(fake, scroller, top);
  assert.equal(fake.room, room - 100);
  assert.equal(fake.scrollTop, top);
  assert.equal(fake.scrollTop, fake.max);
  // Its answer then fills the rest of the room and runs on under the composer, unfollowed.
  for (let grow = 0; grow < 30; grow++) {
    answer.height += 40;
    scroller.resized();
    assert.equal(fake.scrollTop, top);
  }
  assert.equal(scroller.following, false);
  assert.equal(scroller.contentBelow(), true);
  // Going back to the bottom follows it from there.
  userScroll(fake, scroller, fake.max);
  answer.height += 300;
  scroller.resized();
  assert.equal(fake.scrollTop, fake.max);
});

test("a sent turn's room only shrinks: a fold closing in it gives nothing back; a fold opened stops the watch", () => {
  const fake = new Fake();
  conversation(fake, 6);
  const id = key();
  const scroller = open(fake, id, true);
  expectSentTurn(id, "sent");
  const user = fake.row("sent", 60, "user");
  const work = fake.row("work", 40);
  fake.rows.push(user, work);
  scroller.rowsChanged();
  const header = toggle(work, 10);
  scroller.clicked(header);
  work.height += 100;
  scroller.resized();
  const room = fake.room;
  scroller.interrupt();
  scroller.clicked(header);
  work.height -= 100;
  scroller.resized();
  scroller.scrolled();
  assert.equal(fake.room, room);
  // Growing again uses it up again.
  work.height += 50;
  scroller.resized();
  assert.equal(fake.room, room - 50);
  // A fold was clicked: growing past the view is not followed.
  work.height += 1000;
  scroller.resized();
  assert.equal(scroller.following, false);
  assert.equal(scroller.contentBelow(), true);
});

test("in a short view a sent turn ends at the view's end, with no room; a resized view refits the room", () => {
  const fake = new Fake();
  conversation(fake, 6);
  fake.clientHeight = fake.padding + 200;
  const id = key();
  const scroller = open(fake, id, true);
  expectSentTurn(id, "sent");
  const user = fake.row("sent", 60, "user");
  fake.rows.push(user);
  scroller.rowsChanged();
  assert.equal(fake.room, 0);
  assert.equal(fake.scrollTop, fake.max);
  assert.equal(fake.end(user), 200);

  // A tall view shrinking: the room is fitted to it, and the turn's end moves to its line.
  const tall = new Fake();
  conversation(tall, 6);
  const other = key();
  const placing = open(tall, other, true);
  expectSentTurn(other, "sent");
  const sent = tall.row("sent", 60, "user");
  tall.rows.push(sent);
  placing.rowsChanged();
  tall.clientHeight -= 300;
  placing.resized();
  assert.equal(tall.room, roomFor(tall.visible));
  assert.equal(tall.scrollTop, tall.max);
  assert.equal(tall.end(sent), tall.turnLine);
});

test("a row ending in the gap above a grown composer shows no button; one reaching under it does", () => {
  const fake = new Fake();
  conversation(fake, 6);
  const scroller = open(fake);
  // A question docks over the composer: the footer grows, the bottom stays.
  fake.footer += 220;
  scroller.resized();
  const footerTop = () => VIEW_TOP + fake.clientHeight - fake.footer;
  const last = fake.rows.at(-1)!;
  assert.equal(footerTop() - last.getBoundingClientRect().bottom, COMPOSER_GAP);
  // Scrolled up so the last line, still in view, ends inside the gap above it.
  userScroll(fake, scroller, fake.max - (COMPOSER_GAP - 40));
  assert.equal(footerTop() - last.getBoundingClientRect().bottom, 40);
  assert.equal(scroller.contentBelow(), false);
  // Reaching under the footer, it is hidden: the button shows.
  userScroll(fake, scroller, fake.scrollTop - 60);
  assert.ok(last.getBoundingClientRect().bottom > footerTop() + BELOW_PX);
  assert.equal(scroller.contentBelow(), true);
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
  assert.equal(fake.end(fake.rows.at(-1)!), fake.turnLine);
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
  assert.equal(fake.end(edited), fake.turnLine);
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

test("a conversation left at its sent turn's room reopens at the bottom, following, without the room", () => {
  const id = key();
  const first = new Fake();
  conversation(first, 6);
  const scroller = open(first, id, true);
  expectSentTurn(id, "sent");
  first.rows.push(first.row("sent", 60, "user"), first.row("answer", 120));
  scroller.rowsChanged();
  assert.ok(first.room > 0);
  assert.equal(scroller.following, false);

  // Its turn still runs: back at the bottom, following what it adds.
  const again = new Fake();
  conversation(again, 6);
  again.rows.push(again.row("sent", 60, "user"), again.row("answer", 400));
  const reopened = open(again, id, true);
  assert.equal(again.room, 0);
  assert.equal(again.scrollTop, again.max);
  assert.equal(reopened.following, true);
  again.rows.at(-1)!.height += 500;
  reopened.resized();
  assert.equal(again.scrollTop, again.max);

  // Its turn ended while it was away: the bottom too, and nothing below shows the button.
  const ended = new Fake();
  conversation(ended, 6);
  ended.rows.push(ended.row("sent", 60, "user"), ended.row("answer", 120));
  const back = open(ended, id, false);
  assert.equal(ended.room, 0);
  assert.equal(ended.scrollTop, ended.max);
  assert.equal(back.contentBelow(), false);
});

test("a conversation left scrolled away above a sent turn's room reopens at the same place, its room less what grew", () => {
  const id = key();
  const first = new Fake();
  conversation(first, 6);
  const scroller = open(first, id, true);
  expectSentTurn(id, "sent");
  first.rows.push(first.row("sent", 60, "user"), first.row("answer", 120));
  scroller.rowsChanged();
  userScroll(first, scroller, first.scrollTop - 100);
  userScroll(first, scroller, first.scrollTop - 100);
  const room = first.room;
  assert.ok(room > 0);
  const shown = first.rows.find((row) => row.getBoundingClientRect().bottom > VIEW_TOP)!;
  const offset = first.offset(shown);
  const top = first.scrollTop;

  // The same content: the same place, the room kept so it can be reached.
  const again = new Fake();
  conversation(again, 6);
  again.rows.push(again.row("sent", 60, "user"), again.row("answer", 120));
  const reopened = open(again, id, true);
  assert.equal(again.room, room);
  assert.equal(again.scrollTop, top);
  assert.equal(reopened.following, false);

  // Its answer grew by 50px while away: that much of the room is gone, the place is the same.
  const grown = new Fake();
  conversation(grown, 6);
  grown.rows.push(grown.row("sent", 60, "user"), grown.row("answer", 170));
  open(grown, id, true);
  assert.equal(grown.room, room - 50);
  const same = grown.rows.find((row) => row.dataset["messageId"] === shown.dataset["messageId"])!;
  assert.equal(grown.offset(same), offset);
});

test("a sent turn's room never shows the button alone, and what of it shows stays when the turn ends", () => {
  const fake = new Fake();
  conversation(fake, 6);
  const id = key();
  const scroller = open(fake, id, true);
  expectSentTurn(id, "sent");
  const user = fake.row("sent", 60, "user");
  fake.rows.push(user, fake.row("answer", 40));
  scroller.rowsChanged();
  // Streaming: room under the short answer, and no button for the empty room.
  const room = fake.room;
  assert.ok(room > 0);
  assert.equal(scroller.contentBelow(), false);
  // The turn ends: nothing moves, and its answer folding shorter keeps the bottom.
  const top = fake.offset(user);
  scroller.setRunning(false);
  scroller.rowsChanged();
  assert.equal(fake.room, room);
  assert.equal(fake.offset(user), top);
  assert.equal(scroller.contentBelow(), false);
  fake.rows.at(-1)!.height -= 20;
  scroller.scrolled();
  scroller.resized();
  assert.equal(fake.room, room);
  assert.equal(fake.scrollTop, fake.max);
  assert.equal(fake.offset(user), top + 20);
});

test("at the bottom, a resize of the view keeps it at the bottom", () => {
  const fake = new Fake();
  conversation(fake, 10);
  const id = key();
  const scroller = open(fake, id, true);
  // Not following (a sent turn in its room), yet at the bottom.
  expectSentTurn(id, "sent");
  fake.rows.push(fake.row("sent", 60, "user"), fake.row("answer", 100));
  scroller.rowsChanged();
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
  const scroller = open(fake, id, true);
  // Not following (a sent turn in its room), yet at the bottom.
  expectSentTurn(id, "sent");
  fake.rows.push(fake.row("sent", 60, "user"), fake.row("answer", 100));
  scroller.rowsChanged();
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

test("a conversation attached hidden reopens where it was left once shown", () => {
  const id = key();
  const first = new Fake();
  conversation(first, 10);
  userScroll(first, open(first, id), 2000);

  const again = new Fake();
  const { clientHeight, clientWidth } = again;
  again.clientHeight = 0;
  again.clientWidth = 0;
  const scroller = open(again, id);
  conversation(again, 10);
  scroller.rowsChanged();
  again.clientHeight = clientHeight;
  again.clientWidth = clientWidth;
  scroller.resized();
  scroller.scrolled();
  scroller.resized();
  assert.equal(again.scrollTop, 2000);
  assert.equal(scroller.following, false);
});

test("a turn that ended short and works again (a plan approved, a question answered) is followed once it reaches past the bottom", () => {
  const fake = new Fake();
  conversation(fake, 6);
  const id = key();
  const scroller = open(fake, id, true);
  expectSentTurn(id, "sent");
  const user = fake.row("sent", 60, "user");
  const plan = fake.row("plan", 120);
  fake.rows.push(user, plan);
  scroller.rowsChanged();
  // The plan turn ends inside its room: the user reads it at the bottom, nothing follows.
  scroller.setRunning(false);
  scroller.resized();
  assert.ok(fake.room > 0);
  assert.equal(fake.scrollTop, fake.max);
  assert.equal(scroller.following, false);
  // The plan is approved: the same request works again, with no new message of the user's.
  scroller.setRunning(true);
  for (let grow = 0; grow < 30; grow++) {
    plan.height += 40;
    scroller.resized();
  }
  // Its new rows went past the view's bottom: the view follows them.
  assert.equal(scroller.following, true);
  assert.equal(fake.scrollTop, fake.max);
  assert.equal(scroller.contentBelow(), false);
});

test("scrolled up when a short turn works again, the view stays where the user is", () => {
  const fake = new Fake();
  conversation(fake, 6);
  const id = key();
  const scroller = open(fake, id, true);
  expectSentTurn(id, "sent");
  const user = fake.row("sent", 60, "user");
  const plan = fake.row("plan", 120);
  fake.rows.push(user, plan);
  scroller.rowsChanged();
  scroller.setRunning(false);
  scroller.resized();
  userScroll(fake, scroller, fake.scrollTop - 300);
  const top = fake.scrollTop;
  scroller.setRunning(true);
  for (let grow = 0; grow < 30; grow++) {
    plan.height += 40;
    scroller.resized();
  }
  assert.equal(scroller.following, false);
  assert.equal(fake.scrollTop, top);
});

test("a turn that grew past the bottom unheard (a window not painting hears no resize) and ends is still followed", () => {
  const fake = new Fake();
  conversation(fake, 6);
  const id = key();
  const scroller = open(fake, id, true);
  expectSentTurn(id, "sent");
  const user = fake.row("sent", 60, "user");
  const plan = fake.row("plan", 120);
  fake.rows.push(user, plan);
  scroller.rowsChanged();
  scroller.setRunning(false);
  scroller.resized();
  // The plan is approved; its work and answer come while the window is in the background, and
  // the turn ends before the view hears of any of it.
  scroller.setRunning(true);
  plan.height += 1600;
  scroller.setRunning(false);
  scroller.resized();
  assert.equal(scroller.following, true);
  assert.equal(fake.scrollTop, fake.max);
  assert.equal(scroller.contentBelow(), false);
  // A sent turn whose whole answer comes with its end, the same.
  const again = key();
  const other = new Fake();
  conversation(other, 6);
  const second = open(other, again, true);
  expectSentTurn(again, "sent");
  const asked = other.row("sent", 60, "user");
  const answer = other.row("answer", 40);
  other.rows.push(asked, answer);
  second.rowsChanged();
  answer.height += 1600;
  second.setRunning(false);
  second.resized();
  assert.equal(second.following, true);
  assert.equal(other.scrollTop, other.max);
});
