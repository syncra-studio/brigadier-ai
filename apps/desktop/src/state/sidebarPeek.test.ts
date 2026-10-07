import assert from "node:assert/strict";
import { test } from "node:test";

import {
  createPeek,
  PEEK_CLOSE_DELAY_MS,
  PEEK_EXIT_MS,
  PEEK_OPEN_DELAY_MS,
  type PeekPhase,
  type PeekTimers,
} from "@/state/sidebarPeek";

/** Timers on a clock the test moves by hand. */
function fakeClock() {
  let now = 0;
  let next = 0;
  const pending = new Map<number, { at: number; run: () => void }>();
  const timers: PeekTimers = {
    set: (run, ms) => {
      next += 1;
      pending.set(next, { at: now + ms, run });
      return next;
    },
    clear: (handle) => {
      pending.delete(handle as number);
    },
  };
  const advance = (ms: number) => {
    const end = now + ms;
    for (;;) {
      const due = [...pending.entries()]
        .filter(([, timer]) => timer.at <= end)
        .toSorted((a, b) => a[1].at - b[1].at)[0];
      if (!due) break;
      pending.delete(due[0]);
      now = due[1].at;
      due[1].run();
    }
    now = end;
  };
  return { timers, advance };
}

function setup(options: { canOpen?: () => boolean; holdOpen?: () => boolean; exitMs?: number } = {}) {
  const clock = fakeClock();
  const phases: PeekPhase[] = [];
  const peek = createPeek({
    onChange: (phase) => phases.push(phase),
    canOpen: options.canOpen ?? (() => true),
    holdOpen: options.holdOpen ?? (() => false),
    exitMs: () => options.exitMs ?? PEEK_EXIT_MS,
    timers: clock.timers,
  });
  return { peek, phases, advance: clock.advance };
}

test("resting on the trigger opens it after a short delay", () => {
  const { peek, phases, advance } = setup();
  peek.point("trigger");
  advance(PEEK_OPEN_DELAY_MS - 1);
  assert.equal(peek.phase(), "closed");
  // More moves on the trigger don't restart the wait.
  peek.point("trigger");
  advance(1);
  assert.equal(peek.phase(), "open");
  assert.deepEqual(phases, ["open"]);
});

test("passing over the trigger doesn't open it", () => {
  const { peek, advance } = setup();
  peek.point("trigger");
  advance(PEEK_OPEN_DELAY_MS / 2);
  peek.point("outside");
  advance(PEEK_OPEN_DELAY_MS * 2);
  assert.equal(peek.phase(), "closed");
});

test("it doesn't open while something blocks it", () => {
  let blocked = true;
  const { peek, advance } = setup({ canOpen: () => !blocked });
  peek.point("trigger");
  advance(PEEK_OPEN_DELAY_MS);
  assert.equal(peek.phase(), "closed");
  // Once nothing blocks it, a move on the trigger tries again.
  blocked = false;
  peek.point("trigger");
  advance(PEEK_OPEN_DELAY_MS);
  assert.equal(peek.phase(), "open");
});

test("it stays up while the pointer is on the panel, and closes after a grace once it leaves", () => {
  const { peek, phases, advance } = setup();
  peek.point("trigger");
  advance(PEEK_OPEN_DELAY_MS);
  peek.point("panel");
  advance(5_000);
  assert.equal(peek.phase(), "open");
  peek.point("outside");
  advance(PEEK_CLOSE_DELAY_MS - 1);
  assert.equal(peek.phase(), "open");
  advance(1);
  assert.equal(peek.phase(), "closing");
  advance(PEEK_EXIT_MS);
  assert.equal(peek.phase(), "closed");
  assert.deepEqual(phases, ["open", "closing", "closed"]);
});

test("coming back within the grace keeps it open", () => {
  const { peek, advance } = setup();
  peek.point("trigger");
  advance(PEEK_OPEN_DELAY_MS);
  peek.point("outside");
  advance(PEEK_CLOSE_DELAY_MS - 50);
  peek.point("panel");
  advance(PEEK_CLOSE_DELAY_MS * 2);
  assert.equal(peek.phase(), "open");
});

test("the trigger reopens it while it is closing", () => {
  const { peek, phases, advance } = setup();
  peek.point("trigger");
  advance(PEEK_OPEN_DELAY_MS);
  peek.point("outside");
  advance(PEEK_CLOSE_DELAY_MS);
  assert.equal(peek.phase(), "closing");
  peek.point("trigger");
  assert.equal(peek.phase(), "open");
  advance(PEEK_EXIT_MS * 2);
  assert.equal(peek.phase(), "open");
  assert.deepEqual(phases, ["open", "closing", "open"]);
});

test("an open menu or dialog holds it open until it closes", () => {
  let menu = true;
  const { peek, advance } = setup({ holdOpen: () => menu });
  peek.point("trigger");
  advance(PEEK_OPEN_DELAY_MS);
  // The pointer goes to a menu opened from the panel (outside it, in a layer of its own).
  peek.point("outside");
  advance(PEEK_CLOSE_DELAY_MS * 5);
  assert.equal(peek.phase(), "open");
  menu = false;
  advance(PEEK_CLOSE_DELAY_MS);
  assert.equal(peek.phase(), "closing");
});

test("without motion it closes at once", () => {
  const { peek, phases, advance } = setup({ exitMs: 0 });
  peek.point("trigger");
  advance(PEEK_OPEN_DELAY_MS);
  peek.point("outside");
  advance(PEEK_CLOSE_DELAY_MS);
  assert.equal(peek.phase(), "closed");
  assert.deepEqual(phases, ["open", "closed"]);
});

test("dismissed, it stays shut until the pointer has left the trigger", () => {
  const { peek, advance } = setup();
  peek.point("trigger");
  advance(PEEK_OPEN_DELAY_MS);
  peek.point("panel");
  // A click on a chat in the panel navigates, which dismisses it.
  peek.dismiss();
  assert.equal(peek.phase(), "closing");
  advance(PEEK_EXIT_MS);
  assert.equal(peek.phase(), "closed");
  // The pointer is still where the panel was, over the trigger: no reopening.
  peek.point("trigger");
  advance(PEEK_OPEN_DELAY_MS * 3);
  assert.equal(peek.phase(), "closed");
  // Off it and back again opens it as usual.
  peek.point("outside");
  peek.point("trigger");
  advance(PEEK_OPEN_DELAY_MS);
  assert.equal(peek.phase(), "open");
});

test("dismissing cancels a pending open", () => {
  const { peek, advance } = setup();
  peek.point("trigger");
  advance(PEEK_OPEN_DELAY_MS / 2);
  peek.dismiss();
  advance(PEEK_OPEN_DELAY_MS);
  assert.equal(peek.phase(), "closed");
});

test("reset closes it at once, without its exit", () => {
  const { peek, phases, advance } = setup();
  peek.point("trigger");
  advance(PEEK_OPEN_DELAY_MS);
  // The sidebar was opened for real.
  peek.reset();
  assert.equal(peek.phase(), "closed");
  advance(PEEK_CLOSE_DELAY_MS + PEEK_EXIT_MS);
  assert.deepEqual(phases, ["open", "closed"]);
});
