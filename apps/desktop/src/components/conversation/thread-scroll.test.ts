import assert from "node:assert/strict";
import { beforeEach, test } from "node:test";

import {
  EARLIER_PX,
  expectSentTurn,
  FOLLOW_PX,
  jumpInThread,
  keptDistance,
  PLACE_PX,
  roomFor,
  SCROLL_PAD_PX,
  type RowBox,
  SPRING_RATE,
  ThreadScroller,
  TO_BOTTOM_MS,
  wantsEarlier,
} from "@/components/conversation/thread-scroll";

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

  /** The view's height above the scroll padding (the footer and `SCROLL_PAD_PX`). */
  get visible(): number {
    return this.clientHeight - this.footer - SCROLL_PAD_PX;
  }

  /** How far down the view a sent turn's end settles: above the room it is given, 1px from the bottom. */
  get turnLine(): number {
    return this.clientHeight - this.padding - roomFor(this.visible) + 1;
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

/** The user sends a message in `scroller`'s conversation `id`: its row `sent`, then an answer of `answer` px. */
function send(fake: Fake, scroller: ThreadScroller, id: string, answer = 40): { user: Row; reply: Row } {
  expectSentTurn(id, "sent");
  const user = fake.row("sent", 60, "user");
  const reply = fake.row("answer", answer);
  fake.rows.push(user, reply);
  scroller.rowsChanged();
  return { user, reply };
}

/** The view's distance from its bottom. */
function distance(fake: Fake): number {
  return fake.max - fake.scrollTop;
}

/** The user scrolls to `top`. */
function userScroll(fake: Fake, scroller: ThreadScroller, top: number): void {
  scroller.input({ type: "wheel" });
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

test("the button's glide eases out over 260ms and is there once within FOLLOW_PX; while the thread works, or over FOLLOW_PX of room, it jumps there", () => {
  const fake = new Fake();
  conversation(fake, 10);
  const scroller = open(fake);
  userScroll(fake, scroller, 1000);
  const from = distance(fake);
  reduced = false;
  scroller.scrollToBottom();
  assert.equal(scroller.mode, "user_follow");
  const distances: number[] = [];
  let now = performance.now();
  frames.shift()!(now);
  while (frames.length > 0) {
    now += 20;
    frames.shift()!(now);
    distances.push(distance(fake));
  }
  // d(t) = d0·(1 − t/260)³ each frame, until within 24px: then the bottom.
  const model = (ms: number) => from * (1 - Math.min(1, ms / TO_BOTTOM_MS)) ** 3;
  for (const [index, px] of distances.slice(0, -1).entries()) {
    assert.ok(Math.abs(px - model(20 * (index + 1))) < 0.5, `at ${20 * (index + 1)}ms: ${px}`);
    assert.ok(px > FOLLOW_PX);
  }
  assert.equal(distances.at(-1), 0);
  assert.ok(model(20 * distances.length) <= FOLLOW_PX);
  assert.ok(distances.length < 13, "snapped before the full 260ms");

  // Running: a sent turn's room goes, and the view is at the bottom at once.
  const working = new Fake();
  conversation(working, 6);
  const id = key();
  const sending = open(working, id, true);
  send(working, sending, id);
  userScroll(working, sending, 600);
  frames = [];
  sending.scrollToBottom();
  assert.equal(frames.length, 0);
  assert.equal(working.room, 0);
  assert.equal(working.scrollTop, working.max);
  assert.equal(sending.following, true);

  // Idle with more than 24px of room: the same, at once.
  const idle = new Fake();
  conversation(idle, 6);
  const other = key();
  const placed = open(idle, other, true);
  send(idle, placed, other);
  placed.setRunning(false);
  assert.ok(idle.room > FOLLOW_PX);
  frames = [];
  placed.scrollToBottom();
  assert.equal(frames.length, 0);
  assert.equal(idle.room, 0);
  assert.equal(idle.scrollTop, idle.max);
});

test("the button shows once the view is more than FOLLOW_PX above the bottom, the room below aside, and glides there to follow", () => {
  const fake = new Fake();
  conversation(fake, 10);
  const scroller = open(fake);
  userScroll(fake, scroller, fake.max - FOLLOW_PX);
  assert.equal(scroller.contentBelow(), false);
  userScroll(fake, scroller, fake.max - FOLLOW_PX - 1);
  assert.equal(scroller.contentBelow(), true);
  // A grown composer changes nothing of that.
  fake.footer += 220;
  scroller.resized();
  userScroll(fake, scroller, fake.max - FOLLOW_PX);
  assert.equal(scroller.contentBelow(), false);
  userScroll(fake, scroller, 500);
  reduced = false;
  scroller.scrollToBottom();
  // Content grows during the glide: it ends at the new bottom.
  fake.rows.at(-1)!.height += 100;
  flushFrames(scroller);
  assert.equal(fake.scrollTop, fake.max);
  assert.equal(scroller.following, true);

  // A sent turn's room: hidden while the view is within the room and 24px of the bottom.
  reduced = true;
  const sent = new Fake();
  conversation(sent, 6);
  const id = key();
  const placed = open(sent, id, true);
  send(sent, placed, id);
  assert.equal(distance(sent), 1);
  assert.ok(sent.room > 0);
  assert.equal(placed.contentBelow(), false);
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

test("a sent turn springs to a pixel above the bottom, its end a third of the way down, with room its answer fills, then runs on below unfollowed", () => {
  const fake = new Fake();
  conversation(fake, 6);
  const id = key();
  const scroller = open(fake, id, true);
  expectSentTurn(id, "sent");
  const user = fake.row("sent", 60, "user");
  fake.rows.push(user);
  reduced = false;
  scroller.rowsChanged();
  // The spring: critically damped at 18.47 rad/s (bounce 0, 0.5s), no overshoot, to d = 1.
  assert.equal(SPRING_RATE, 18.47);
  const goal = () => fake.max - 1;
  const from = fake.scrollTop;
  const left: number[] = [];
  let now = performance.now();
  frames.shift()!(now);
  while (frames.length > 0) {
    now += 10;
    frames.shift()!(now);
    left.push((goal() - fake.scrollTop) / (goal() - from));
  }
  const t = (SPRING_RATE * 50) / 1000;
  assert.ok(left.every((share, index) => share >= 0 && (index === 0 || share <= left[index - 1]!)));
  assert.ok(Math.abs(left[4]! - (1 + t) * Math.exp(-t)) < 0.01, `at 50ms: ${left[4]}`);
  assert.ok(left.length <= 60, `settled after ${left.length * 10}ms`);
  assert.ok(left[49]! * (goal() - from) < 1, `at 500ms: ${left[49]! * (goal() - from)}px left`);
  assert.equal(distance(fake), 1);
  // Its end sits a third of the view down; the room reaches down to the pad.
  assert.equal(fake.end(user), fake.turnLine);
  assert.equal(fake.room, roomFor(fake.visible));
  assert.equal(scroller.mode, "prework_watch");
  // The answer streams into the room: nothing moves, the room shrinking by each growth.
  const answer = fake.row("answer", 40);
  fake.rows.push(answer);
  scroller.rowsChanged();
  const top = fake.scrollTop;
  const offset = fake.offset(user);
  while (fake.room > 0) {
    const room = fake.room;
    answer.height += 40;
    scroller.resized();
    if (fake.room > 0) assert.equal(fake.room, room - 40);
    assert.equal(fake.scrollTop, top);
    assert.equal(fake.offset(user), offset);
  }
  // Past it the answer grows below the view: no autoscroll, and the button shows.
  for (let grow = 0; grow < 10; grow++) {
    answer.height += 40;
    scroller.resized();
    assert.equal(fake.scrollTop, top);
  }
  assert.equal(scroller.following, false);
  assert.ok(distance(fake) > FOLLOW_PX);
  assert.equal(scroller.contentBelow(), true);
});

test("the room is max(0, min(avail × 2/3, avail − 240)) of the view above the footer and SCROLL_PAD_PX, as tall as when sent", () => {
  // The spec's geometry: a 976px view under a 237.94px composer.
  assert.equal(SCROLL_PAD_PX, 16);
  assert.ok(Math.abs(roomFor(976 - 237.94 - 16) - 481.3733) < 0.001);
  // The crossover is at 720px: below it, avail − 240.
  assert.equal(roomFor(720), 480);
  assert.equal(roomFor(600), 360);
  assert.equal(roomFor(200), 0);
  const fake = new Fake();
  conversation(fake, 6);
  fake.footer = 237.94;
  const id = key();
  const scroller = open(fake, id, true);
  send(fake, scroller, id, 0);
  assert.ok(Math.abs(fake.room + 0 - roomFor(722.06)) < 0.001);
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

test("scrolled up from a sent turn, the room shrinks by as much and goes once used up; the turn is not followed; back at the bottom, it is", () => {
  const fake = new Fake();
  conversation(fake, 6);
  const id = key();
  const scroller = open(fake, id, true);
  const { reply: answer } = send(fake, scroller, id);
  const room = fake.room;
  // 100px up: the room shrinks by 100px, so the view stays 1px above the new bottom.
  const top = fake.scrollTop - 100;
  userScroll(fake, scroller, top);
  assert.equal(fake.room, room - 100);
  assert.equal(fake.scrollTop, top);
  assert.equal(distance(fake), 1);
  // Further up than the room is tall: it is gone.
  userScroll(fake, scroller, fake.scrollTop - 600);
  assert.equal(fake.room, 0);
  const at = fake.scrollTop;
  // Its answer then runs on under the composer, unfollowed.
  for (let grow = 0; grow < 30; grow++) {
    answer.height += 40;
    scroller.resized();
    assert.equal(fake.scrollTop, at);
  }
  assert.equal(scroller.following, false);
  assert.equal(scroller.contentBelow(), true);
  // Going back to the bottom from further than 24px follows it from there.
  userScroll(fake, scroller, fake.max);
  assert.equal(scroller.mode, "user_follow");
  answer.height += 300;
  scroller.resized();
  assert.equal(fake.scrollTop, fake.max);
});

test("the user's own scroll to the very bottom clears the room; so does content that fits without it at the bottom", () => {
  const fake = new Fake();
  conversation(fake, 6);
  const id = key();
  const scroller = open(fake, id, true);
  send(fake, scroller, id);
  assert.ok(fake.room > 0);
  // From 1px above the bottom, a wheel down to the very bottom.
  userScroll(fake, scroller, fake.max);
  assert.equal(fake.room, 0);
  // Coming from within 24px, it doesn't follow by that.
  assert.equal(scroller.following, false);

  const short = new Fake();
  short.rows.push(short.row("u0", 60, "user"), short.row("a0", 100));
  const other = key();
  const scroller2 = open(short, other);
  expectSentTurn(other, "sent");
  short.rows.push(short.row("sent", 60, "user"));
  scroller2.rowsChanged();
  assert.ok(short.room > 0);
  // The browser's scroll at the very bottom, with content that fits the view without the room.
  short.scrollTop = short.max;
  scroller2.scrolled();
  assert.equal(short.room, 0);
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

test("in a short view a sent turn gets no room; a resize trims the room to the new size and never grows it back", () => {
  const fake = new Fake();
  conversation(fake, 6);
  fake.clientHeight = fake.footer + SCROLL_PAD_PX + 200;
  const id = key();
  const scroller = open(fake, id, true);
  expectSentTurn(id, "sent");
  fake.rows.push(fake.row("sent", 60, "user"));
  scroller.rowsChanged();
  assert.equal(fake.room, 0);
  assert.equal(distance(fake), 1);

  // The spec's numbers: a 976px view under a 237.94px composer shrinks to 652px and back.
  const tall = new Fake();
  conversation(tall, 6);
  tall.footer = 237.94;
  const other = key();
  const placing = open(tall, other, true);
  expectSentTurn(other, "sent");
  tall.rows.push(tall.row("sent", 60, "user"));
  placing.rowsChanged();
  assert.ok(Math.abs(tall.room - 481.3733) < 0.001);
  tall.clientHeight = 652;
  placing.resized();
  assert.ok(Math.abs(tall.room - 158.06) < 0.001, `${tall.room}`);
  assert.equal(distance(tall), 0);
  tall.clientHeight = 976;
  placing.resized();
  assert.ok(Math.abs(tall.room - 158.06) < 0.001, `${tall.room}`);
  // A taller composer trims it too.
  tall.footer = 600;
  placing.resized();
  assert.equal(tall.room, roomFor(976 - 600 - SCROLL_PAD_PX));
  assert.ok(tall.room < 158.06);
});

test("a message is placed only when sent within PLACE_PX of the bottom, the room aside; from further up nothing moves and the room goes", () => {
  assert.equal(PLACE_PX, 300);
  for (const [up, placed] of [[300, true], [301, false]] as const) {
    const fake = new Fake();
    conversation(fake, 10);
    const id = key();
    const scroller = open(fake, id, true);
    userScroll(fake, scroller, fake.max - up);
    const top = fake.scrollTop;
    expectSentTurn(id, "sent");
    fake.rows.push(fake.row("sent", 60, "user"));
    scroller.rowsChanged();
    if (placed) {
      assert.ok(fake.room > 0);
      assert.equal(distance(fake), 1);
    } else {
      assert.equal(fake.room, 0);
      assert.equal(fake.scrollTop, top);
      assert.equal(scroller.mode, "static");
    }
  }
  // The room under the last turn doesn't count against it: 300px past it is still placed.
  const fake = new Fake();
  conversation(fake, 6);
  const id = key();
  const scroller = open(fake, id, true);
  send(fake, scroller, id);
  scroller.setRunning(false);
  const room = fake.room;
  // The room below the view, with the view 300px above where the room starts (no scroll to get
  // there would leave the room this tall: rule 12 takes it as the user scrolls up).
  fake.scrollTop = fake.max - room - 300;
  scroller.scrolled();
  expectSentTurn(id, "next");
  fake.rows.push(fake.row("next", 60, "user"));
  scroller.rowsChanged();
  assert.equal(distance(fake), 1);
});

test("the first turn of a thread gets no room and no spring: it is followed from the bottom as it grows", () => {
  const fake = new Fake();
  const id = key();
  const scroller = open(fake, id);
  expectSentTurn(id, "first");
  const user = fake.row("first", 60, "user");
  fake.rows.push(user);
  scroller.rowsChanged();
  assert.equal(fake.scrollTop, 0);
  assert.equal(fake.offset(user), TOP_PAD);
  assert.equal(fake.room, 0);
  assert.equal(scroller.mode, "user_follow");
  scroller.setRunning(true);
  const answer = fake.row("answer", 40);
  fake.rows.push(answer);
  scroller.rowsChanged();
  for (let grow = 0; grow < 40; grow++) {
    answer.height += 40;
    scroller.resized();
    assert.equal(fake.scrollTop, fake.max);
  }
  assert.ok(fake.max > 0);
});

test("the first message sent from the new-chat view is followed in the conversation it starts; one sent before a thread mounts is placed", () => {
  const id = key();
  expectSentTurn(id, "draft");
  const fake = new Fake();
  fake.rows.push(fake.row("draft", 60, "user"));
  const scroller = open(fake, id);
  assert.equal(scroller.following, true);
  assert.equal(fake.room, 0);

  const other = key();
  expectSentTurn(other, "draft");
  const later = new Fake();
  conversation(later, 3);
  later.rows.push(later.row("draft", 60, "user"));
  const placed = open(later, other);
  assert.equal(placed.following, false);
  assert.ok(later.room > 0);
  assert.equal(later.end(later.rows.at(-1)!), later.turnLine);

  // Rows mounted after a spacer for messages not mounted are no first turn.
  const deep = new Fake();
  const third = key();
  const layout = deep.layout();
  const scroller3 = new ThreadScroller();
  scroller3.attach({ ...layout, before: () => true }, third);
  expectSentTurn(third, "sent");
  deep.rows.push(deep.row("sent", 60, "user"));
  scroller3.rowsChanged();
  assert.equal(scroller3.following, false);
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

test("a conversation reopens at the distance from the bottom it was left at", () => {
  const id = key();
  const first = new Fake();
  conversation(first, 10);
  const scroller = open(first, id);
  userScroll(first, scroller, 2345);
  const left = distance(first);

  const again = new Fake();
  conversation(again, 10);
  const back = open(again, id);
  assert.equal(again.scrollTop, 2345);
  assert.equal(back.mode, "static");

  // Earlier rows loaded above it, and later ones below, while it was away: the same distance.
  const grown = new Fake();
  grown.rows.push(grown.row("older-u", 60, "user"), grown.row("older-a", 400));
  conversation(grown, 10);
  conversation(grown, 2, "new-");
  const reopened = open(grown, id);
  assert.equal(distance(grown), left);
  assert.equal(reopened.following, false);

  // Within 24px of the bottom, it comes back at the bottom, following.
  const near = key();
  const third = new Fake();
  conversation(third, 10);
  userScroll(third, open(third, near), third.max - FOLLOW_PX);
  const fourth = new Fake();
  conversation(fourth, 12);
  const atBottom = open(fourth, near);
  assert.equal(fourth.scrollTop, fourth.max);
  assert.equal(atBottom.mode, "user_follow");
});

test("a reopened conversation keeps asking for its distance while its content loads", () => {
  const id = key();
  const first = new Fake();
  conversation(first, 10);
  const scroller = open(first, id);
  userScroll(first, scroller, 1000);
  const left = distance(first);

  // It shows collapsed (its replies still loading): the place can't be reached yet.
  const loading = new Fake();
  conversation(loading, 10);
  for (const row of loading.rows) row.height = 30;
  const reopened = open(loading, id);
  assert.ok(loading.max < left);
  for (const row of loading.rows) if (row.dataset["role"] === "assistant") row.height = 400;
  reopened.resized();
  assert.equal(distance(loading), left);
  assert.equal(reopened.following, false);
  // Rows above it measured later keep what shows; so does a row below it.
  const shown = loading.rows.find((row) => row.getBoundingClientRect().bottom > VIEW_TOP)!;
  const offset = loading.offset(shown);
  loading.rows[0]!.height += 300;
  reopened.resized();
  assert.equal(loading.offset(shown), offset);
  loading.rows.at(-1)!.height += 300;
  reopened.resized();
  assert.equal(loading.offset(shown), offset);

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

test("a long thread that grew while away reopens at its distance; its turn growing below the view after moves nothing", () => {
  const id = key();
  const first = new Fake();
  conversation(first, 30);
  const scroller = open(first, id, true);
  userScroll(first, scroller, first.max - 3000);
  const left = distance(first);

  // Its turn went on while away: two more turns, and the newest still growing once shown.
  const grown = new Fake();
  conversation(grown, 32);
  const reopened = open(grown, id, true);
  assert.equal(distance(grown), left);
  const top = grown.scrollTop;
  grown.rows.at(-1)!.height += 900;
  reopened.resized();
  assert.equal(grown.scrollTop, top);
  assert.equal(distance(grown), left + 900);
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

test("a place is kept as its distance with the room left out: at the bottom while more than FOLLOW_PX of room shows above the padding", () => {
  // keptDistance(d, room, padding): (room − d − padding > 24) ? 0 : max(0, d − room).
  assert.equal(keptDistance(1, 481, 254), 0);
  assert.equal(keptDistance(1, 279, 254), 0);
  assert.equal(keptDistance(1, 278, 254), 0);
  assert.equal(keptDistance(1, 279.5, 254), 0);
  assert.equal(keptDistance(200, 300, 254), 0);
  assert.equal(keptDistance(443, 0, 254), 443);
  assert.equal(keptDistance(600, 158, 254), 442);
  assert.equal(keptDistance(5, 270, 254), 0);
  assert.equal(keptDistance(30, 270, 254), 0);

  // Rows replaced while it was away: the same distance still.
  const other = key();
  const elsewhere = new Fake();
  conversation(elsewhere, 10);
  const left = open(elsewhere, other);
  userScroll(elsewhere, left, 2000);
  const kept = distance(elsewhere);
  const none = new Fake();
  conversation(none, 10, "replaced-");
  const reopened = open(none, other);
  assert.equal(distance(none), kept);
  assert.equal(reopened.following, false);
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

test("a conversation left above a sent turn's room reopens at its distance with the room left out, and no room", () => {
  const id = key();
  const first = new Fake();
  conversation(first, 6);
  const scroller = open(first, id, true);
  send(first, scroller, id, 120);
  // A little up: the room still shows, so it comes back at the bottom.
  userScroll(first, scroller, first.scrollTop - 100);
  assert.ok(first.room > FOLLOW_PX);
  const again = new Fake();
  conversation(again, 6);
  again.rows.push(again.row("sent", 60, "user"), again.row("answer", 120));
  const reopened = open(again, id, true);
  assert.equal(again.room, 0);
  assert.equal(again.scrollTop, again.max);
  assert.equal(reopened.mode, "user_follow");

  // Far up: the room is gone; it comes back that far from the bottom.
  const other = key();
  const second = new Fake();
  conversation(second, 6);
  const scroller2 = open(second, other, true);
  send(second, scroller2, other, 120);
  userScroll(second, scroller2, second.scrollTop - 1500);
  assert.equal(second.room, 0);
  const left = distance(second);
  const back = new Fake();
  conversation(back, 6);
  back.rows.push(back.row("sent", 60, "user"), back.row("answer", 120));
  open(back, other, true);
  assert.equal(back.room, 0);
  assert.equal(distance(back), left);
});

test("a sent turn's room never shows the button alone, and stays as it is when the turn ends", () => {
  const fake = new Fake();
  conversation(fake, 6);
  const id = key();
  const scroller = open(fake, id, true);
  const { user } = send(fake, scroller, id);
  const room = fake.room;
  assert.ok(room > 0);
  assert.equal(scroller.contentBelow(), false);
  // The turn ends: nothing moves, the room is kept, and the view is static.
  const top = fake.offset(user);
  scroller.setRunning(false);
  scroller.rowsChanged();
  assert.equal(fake.room, room);
  assert.equal(fake.offset(user), top);
  assert.equal(scroller.contentBelow(), false);
  assert.equal(scroller.mode, "static");
  // Its answer folding shorter gives nothing back to the room: the browser clamps the view.
  fake.rows.at(-1)!.height -= 20;
  scroller.scrolled();
  scroller.resized();
  assert.equal(fake.room, room);
  assert.equal(fake.scrollTop, fake.max);
  assert.equal(fake.offset(user), top + 19);
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

test("work that starts again (a plan approved) is watched: followed only if it reaches past the view while more than FOLLOW_PX of room is left", () => {
  const fake = new Fake();
  conversation(fake, 6);
  const id = key();
  const scroller = open(fake, id, true);
  const { reply: plan } = send(fake, scroller, id, 120);
  // The plan turn ends inside its room: the user reads it, nothing follows.
  scroller.setRunning(false);
  scroller.resized();
  assert.equal(scroller.mode, "static");
  const room = fake.room;
  assert.ok(room > FOLLOW_PX);
  // The plan is approved: the same request works again, with no new message of the user's.
  scroller.setRunning(true);
  assert.equal(scroller.mode, "prework_watch");
  // Growing a little at a time, it uses the room up and runs on below: not followed.
  const top = fake.scrollTop;
  for (let grow = 0; grow < 30; grow++) {
    plan.height += 40;
    scroller.resized();
  }
  assert.equal(fake.room, 0);
  assert.equal(fake.scrollTop, top);
  assert.equal(scroller.following, false);
  assert.equal(scroller.contentBelow(), true);

  // Growing past the view in one go while room is left: the room goes and it is followed.
  const jump = new Fake();
  conversation(jump, 6);
  const other = key();
  const watched = open(jump, other, true);
  const { reply } = send(jump, watched, other, 40);
  reply.height += jump.room + 500;
  watched.resized();
  assert.equal(watched.mode, "prework_follow");
  assert.equal(jump.room, 0);
  assert.equal(jump.scrollTop, jump.max);
  assert.equal(watched.contentBelow(), false);
});

test("the follow modes change as the spec's table says", () => {
  const fake = new Fake();
  conversation(fake, 6);
  const id = key();
  const scroller = open(fake, id);
  // Opened at the bottom: following.
  assert.equal(scroller.mode, "user_follow");
  // Placed: static; its work starting (already, or after), watched.
  scroller.setRunning(true);
  send(fake, scroller, id);
  assert.equal(scroller.mode, "prework_watch");
  // The answer of an older turn starting says nothing; the newest's ends the watch.
  scroller.answerStarted(["u0"]);
  assert.equal(scroller.mode, "prework_watch");
  scroller.answerStarted(["sent"]);
  assert.equal(scroller.mode, "static");
  scroller.setRunning(false);
  assert.equal(scroller.mode, "static");

  // prework_follow becomes user_follow when the answer starts.
  const other = new Fake();
  conversation(other, 6);
  const second = key();
  const s2 = open(other, second, true);
  const { reply } = send(other, s2, second);
  reply.height += other.room + 500;
  s2.resized();
  assert.equal(s2.mode, "prework_follow");
  // A 12px nudge up keeps it; 40px up, still in prework, goes back to watching.
  userScroll(other, s2, other.scrollTop - 12);
  assert.equal(s2.mode, "prework_follow");
  userScroll(other, s2, other.max);
  userScroll(other, s2, other.scrollTop - 40);
  assert.equal(s2.mode, "prework_watch");
  s2.scrollToBottom();
  assert.equal(s2.mode, "user_follow");
  s2.answerStarted(["sent"]);
  assert.equal(s2.mode, "user_follow");
  // After the answer started, 40px up is static.
  userScroll(other, s2, other.scrollTop - 40);
  assert.equal(s2.mode, "static");
  // Back to within 24px from further, with no room: following again.
  userScroll(other, s2, other.max - 10);
  assert.equal(s2.mode, "user_follow");

  // A user scroll back to the bottom with more than 24px of room left doesn't follow.
  const third = new Fake();
  conversation(third, 6);
  const id3 = key();
  const s3 = open(third, id3, true);
  send(third, s3, id3);
  third.scrollTop = third.max - 400;
  s3.scrolled();
  const room = third.room;
  assert.ok(room > FOLLOW_PX);
  userScroll(third, s3, third.max - 10);
  assert.notEqual(s3.mode, "user_follow");
});

test("a turn followed to its end is followed through its own last layout, then the view is static", () => {
  const now = performance.now;
  let clock = 50_000;
  performance.now = () => clock;
  try {
    const fake = new Fake();
    conversation(fake, 6);
    const scroller = open(fake, key(), true);
    assert.equal(scroller.mode, "user_follow");
    scroller.setRunning(false);
    // Its actions row comes with its end: followed.
    clock += 16;
    fake.rows.at(-1)!.height += 32;
    scroller.resized();
    assert.equal(fake.scrollTop, fake.max);
    // Later, a late line: not followed.
    clock += 1_000;
    const top = fake.scrollTop;
    fake.rows.at(-1)!.height += 60;
    scroller.resized();
    assert.equal(scroller.mode, "static");
    assert.equal(fake.scrollTop, top);
  } finally {
    performance.now = now;
  }
});

test("only a mostly vertical wheel without Ctrl or Shift, a touch past its slop, a press on the scrollbar or a scroll key outside a field is the user's scroll", () => {
  const cases: [Parameters<ThreadScroller["input"]>[0], boolean][] = [
    [{ type: "wheel", deltaX: 0, deltaY: -40 }, true],
    [{ type: "wheel", deltaX: 40, deltaY: -40 }, false],
    [{ type: "wheel", deltaX: 0, deltaY: -40, ctrlKey: true }, false],
    [{ type: "wheel", deltaX: 0, deltaY: -40, shiftKey: true }, false],
    [{ type: "keydown", key: "PageUp" }, true],
    [{ type: "keydown", key: "a" }, false],
  ];
  for (const [event, users] of cases) {
    const fake = new Fake();
    conversation(fake, 10);
    const scroller = open(fake);
    scroller.input(event);
    fake.scrollTop = fake.max - 300;
    scroller.scrolled();
    assert.equal(scroller.following, !users, JSON.stringify(event));
    assert.equal(fake.scrollTop, users ? fake.max - 300 : fake.max, JSON.stringify(event));
  }
  // A touch: not by 8px, by 9px it is.
  for (const [moved, users] of [[8, false], [9, true]] as const) {
    const fake = new Fake();
    conversation(fake, 10);
    const scroller = open(fake);
    scroller.input({ type: "touchstart", touches: [{ clientY: 500 }] });
    scroller.input({ type: "touchmove", touches: [{ clientY: 500 + moved }] });
    fake.scrollTop = fake.max - 300;
    scroller.scrolled();
    assert.equal(scroller.following, !users, `touch ${moved}px`);
  }
  // A press: on the scroll element itself (its scrollbar), not on content.
  for (const [onViewport, users] of [[true, true], [false, false]] as const) {
    const fake = new Fake();
    conversation(fake, 10);
    const scroller = open(fake);
    scroller.input({ type: "pointerdown", target: (onViewport ? fake : {}) as unknown as EventTarget });
    fake.scrollTop = fake.max - 300;
    scroller.scrolled();
    assert.equal(scroller.following, !users, `pressed on the ${onViewport ? "scrollbar" : "content"}`);
  }
});

test("any wheel, touch, press or key stops a sent turn's spring, whether it scrolls or not", () => {
  for (const event of [{ type: "wheel", deltaX: 40, deltaY: 0 }, { type: "touchstart" }, { type: "pointerdown", target: {} as EventTarget }, { type: "keydown", key: "a" }]) {
    const fake = new Fake();
    conversation(fake, 6);
    const id = key();
    const scroller = open(fake, id, true);
    reduced = false;
    send(fake, scroller, id);
    assert.ok(frames.length > 0);
    scroller.input(event);
    const top = fake.scrollTop;
    for (const frame of frames.splice(0)) frame(performance.now() + 100);
    assert.equal(fake.scrollTop, top, event.type);
    reduced = true;
  }
});

test("older messages load once the user scrolls up to within EARLIER_PX of the top; not scrolling down, nor by code", () => {
  assert.equal(EARLIER_PX, 64);
  const fake = new Fake();
  conversation(fake, 10);
  const scroller = open(fake);
  const view = fake as unknown as Element;
  userScroll(fake, scroller, 65);
  assert.equal(wantsEarlier(view), false);
  userScroll(fake, scroller, 64);
  assert.equal(wantsEarlier(view), true);
  userScroll(fake, scroller, 30);
  userScroll(fake, scroller, 40);
  assert.equal(wantsEarlier(view), false);
  // A jump by code (Find, a reveal) to the top, right after the user's wheel.
  userScroll(fake, scroller, 500);
  jumpInThread({ closest: () => fake } as unknown as Element);
  fake.scrollTop = 10;
  scroller.scrolled();
  assert.equal(wantsEarlier(view), false);
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

test("a clamp heard only after the content grew back (no input of the user's) keeps a following view at its bottom", () => {
  const fake = new Fake();
  conversation(fake, 6);
  const scroller = open(fake);
  const plan = fake.row("plan", 600);
  fake.rows.push(fake.row("asked", 60, "user"), plan);
  scroller.rowsChanged();
  assert.equal(scroller.following, true);
  assert.equal(fake.scrollTop, fake.max);
  // A plan approved: its turn is laid out again as its work starts, shorter for a moment, and
  // the browser clamps the view; the turn is back, taller, before the scroll is heard.
  plan.height = 100;
  fake.scrollTop = fake.max;
  plan.height = 700;
  scroller.scrolled();
  assert.equal(scroller.following, true);
  assert.equal(fake.scrollTop, fake.max);
  scroller.setRunning(true);
  plan.height += 400;
  scroller.resized();
  assert.equal(fake.scrollTop, fake.max);
  assert.equal(scroller.contentBelow(), false);
  // The user's own wheel up still stops following.
  userScroll(fake, scroller, fake.scrollTop - 300);
  assert.equal(scroller.following, false);
});

test("growth heard first by the view's own scroll, before its resize, is still followed", () => {
  const fake = new Fake();
  conversation(fake, 6);
  const scroller = open(fake, key(), true);
  const answer = fake.rows.at(-1)!;
  answer.height += 23;
  scroller.resized();
  assert.equal(fake.scrollTop, fake.max);
  // The next line of the answer is laid out before the frame runs: its scroll event (the move
  // above) comes first, its resize after.
  answer.height += 34;
  scroller.scrolled();
  scroller.resized();
  assert.equal(scroller.following, true);
  assert.equal(fake.scrollTop, fake.max);
});

test("a jump the code makes for the user (a message revealed) stays; a scroll with no input of theirs goes back while following", () => {
  const fake = new Fake();
  conversation(fake, 10);
  const scroller = open(fake);
  assert.equal(scroller.following, true);
  fake.scrollTop = fake.max - 500;
  scroller.scrolled();
  assert.equal(fake.scrollTop, fake.max);
  assert.equal(scroller.following, true);
  // A message revealed: the code says so first, and the view stays where it moved.
  jumpInThread({ closest: () => fake } as unknown as Element);
  fake.scrollTop = 800;
  scroller.scrolled();
  assert.equal(fake.scrollTop, 800);
  assert.equal(scroller.following, false);
});

test("a jump's move is kept when the content resizes before its scroll is heard", () => {
  const fake = new Fake();
  conversation(fake, 10);
  const scroller = open(fake);
  assert.equal(fake.scrollTop, fake.max);
  // A message revealed from a frame callback: the view moved, and in the same frame the content
  // below it shrinks, its resize heard before the jump's scroll (which comes with the next frame).
  jumpInThread({ closest: () => fake } as unknown as Element);
  fake.scrollTop = 800;
  fake.rows.at(-1)!.height = 100;
  scroller.resized();
  assert.equal(fake.scrollTop, 800);
  scroller.scrolled();
  assert.equal(fake.scrollTop, 800);
  assert.equal(scroller.following, false);
});

test("a fling that goes on long after the user's touch is still theirs, and reaching the bottom follows again", () => {
  const now = performance.now;
  let clock = 10_000;
  performance.now = () => clock;
  try {
    const fake = new Fake();
    conversation(fake, 10);
    const scroller = open(fake);
    userScroll(fake, scroller, 1000);
    assert.equal(scroller.following, false);
    // The fling glides on, a scroll every frame, well past a second after the touch.
    for (let top = 1000; top < fake.max; top = Math.min(fake.max, top + 20)) {
      clock += 16;
      fake.scrollTop = top + 20;
      scroller.scrolled();
    }
    assert.ok(clock - 10_000 > 1_000);
    assert.equal(scroller.following, true);
    // A quiet moment later, a scroll with no input is the browser's: a following view stays at its bottom.
    clock += 2_000;
    fake.scrollTop = fake.max - 300;
    scroller.scrolled();
    assert.equal(fake.scrollTop, fake.max);
  } finally {
    performance.now = now;
  }
});

test("a focus moved into the thread from outside it keeps the scroll that brings it into view", () => {
  const fake = new Fake();
  conversation(fake, 10);
  const scroller = open(fake);
  assert.equal(scroller.following, true);
  // The latest response's pill, from the keyboard in the composer: the answer is focused, and the
  // browser scrolls up to it.
  scroller.focused();
  fake.scrollTop = 1200;
  scroller.scrolled();
  assert.equal(fake.scrollTop, 1200);
  assert.equal(scroller.following, false);
});
