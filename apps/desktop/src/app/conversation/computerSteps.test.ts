import assert from "node:assert/strict";
import { test } from "node:test";

import {
  actionWords,
  batchesOf,
  foldComputer,
  isComputerItem,
  outcomeWords,
  playbackKey,
  playStep,
  playToggle,
  routeWords,
  summaryWords,
} from "@/app/conversation/computerSteps";
import type { ActionItem } from "@/app/conversation/activity/words";
import type { ThreadEntry } from "@/components/transcript/activity";
import type { ComputerAction } from "@/ipc/generated";
import { withAction } from "@/state/board";

const tool = (key: string, name: string): ActionItem => ({ kind: "tool", key, name, input: "{}", status: "completed", output: null });
const command = (key: string, text: string): ActionItem => ({
  kind: "command", key, command: text, cwd: "/tmp", status: "completed", exitCode: 0, output: "", durationMs: 10,
} as ActionItem);

export function action(over: Partial<ComputerAction>): ComputerAction {
  return {
    batch: "b1", index: 0, atMs: 1000, kind: "click", app: "Target Range", appWindow: "Target Range",
    target: 'button "Button 8 pt"', pid: 7, window: 3, status: "done", rung: "element", effect: "confirmed",
    error: null, detail: null, dispatchMs: 2.4, record: "{}", image: null, ...over,
  };
}

test("both CLIs' computer tools and the computer CLI are computer calls; other tools aren't", () => {
  assert.ok(isComputerItem(tool("a", "mcp__computer__act")));
  assert.ok(isComputerItem(tool("b", "computer/observe")));
  assert.ok(isComputerItem(tool("c", "computer.zoom")));
  assert.ok(isComputerItem(command("d", '"$BRIGADIER_COMPUTER_CLI" computer act --window 3')));
  assert.ok(isComputerItem(command("e", "/x/brigadierd computer observe 3")));
  assert.ok(!isComputerItem(tool("f", "mcp__brigadier__submit_report")));
  assert.ok(!isComputerItem(tool("g", "Read")));
  assert.ok(!isComputerItem(command("h", "echo computer act")));
});

test("the computer calls fold out of the activity into one list, and empty action runs go", () => {
  const message: ThreadEntry = { kind: "item", item: { kind: "message", key: "m", role: "assistant", text: "Looking.", streaming: false } as never };
  const entries: ThreadEntry[] = [
    { kind: "actions", key: "a1", items: [tool("r", "Read"), tool("o", "mcp__computer__observe"), tool("x", "mcp__computer__act")] },
    message,
    { kind: "actions", key: "a2", items: [tool("x2", "mcp__computer__act")] },
  ];
  const { entries: kept, calls } = foldComputer(entries);
  assert.deepEqual(calls.map((call) => call.key), ["o", "x", "x2"]);
  assert.equal(kept.length, 2);
  assert.deepEqual(kept[0], { kind: "actions", key: "a1", items: [tool("r", "Read")] });
  assert.equal(kept[1], message);
});

test("actions group by batch and read in plain words, outcome first, route and time in the details", () => {
  const batches = batchesOf([
    action({ index: 0, image: "h1" }),
    action({ index: 1, kind: "type", target: "textfield", rung: "background", effect: "unverified" }),
    action({ batch: "b2", index: 0, kind: "menu", target: "File › Save", atMs: 2000 }),
    action({ batch: "b2", index: 1, kind: "key", target: "cmd+s", status: "failed", effect: null, error: "stale_ref", detail: "e4 is gone", atMs: 2001 }),
    action({ batch: "b2", index: 2, status: "skipped", effect: null, rung: null, atMs: 2002 }),
  ]);
  assert.deepEqual(batches.map((b) => [b.key, b.actions.length, b.image]), [["b1", 2, "h1"], ["b2", 3, null]]);
  const [b1, b2] = batches;
  assert.equal(actionWords(b1!.actions[0]!), "Clicked “Button 8 pt”");
  assert.equal(actionWords(b1!.actions[1]!), "Typed into the text field");
  assert.equal(actionWords(b2!.actions[0]!), "Chose File › Save");
  assert.equal(actionWords(b2!.actions[1]!), "Pressed cmd+s");
  assert.equal(actionWords(action({ target: null, record: '{"action":{"do":"click","count":2}}' })), "Double-clicked a point");
  assert.deepEqual(outcomeWords(b1!.actions[0]!), { text: "worked", failed: false });
  assert.deepEqual(outcomeWords(b2!.actions[1]!), { text: "failed: e4 is gone", failed: true });
  assert.deepEqual(outcomeWords(b2!.actions[2]!), { text: "skipped", failed: false });
  assert.equal(routeWords(b1!.actions[0]!), "Directly, through the app's accessibility · Target Range, “Target Range” · 2 ms");
  assert.equal(routeWords(b1!.actions[1]!).startsWith("In the background"), true);
  const page = action({ kind: "navigate", target: "http://localhost:8080/", rung: "page" });
  assert.equal(actionWords(page), "Went to http://localhost:8080/");
  assert.equal(routeWords(page).startsWith("Inside the page, through the browser"), true);
  assert.equal(summaryWords(b1!.actions, false), "Used the computer · 2 steps in Target Range");
  assert.equal(summaryWords(b1!.actions, true), "Using the computer · Typed into the text field in Target Range");
});

const at = (index: number, playing = false) => ({ index, playing });

test("the keys step, jump and play; playback stops at the last batch", () => {
  assert.deepEqual(playbackKey(at(2), "ArrowLeft", 5), at(1));
  assert.deepEqual(playbackKey(at(4), "ArrowRight", 5), at(4));
  assert.deepEqual(playbackKey(at(2), "Home", 5), at(0));
  assert.deepEqual(playbackKey(at(0), "End", 5), at(4));
  assert.equal(playbackKey(at(0), "a", 5), null);
  // Play at the end starts again from the first batch.
  assert.deepEqual(playToggle(at(4), 5), at(0, true));
  assert.deepEqual(playToggle(at(1, true), 5), at(1));
  assert.deepEqual(playStep(at(2, true), 5), at(3, true));
  assert.deepEqual(playStep(at(3, true), 5), at(4, false));
  assert.deepEqual(playToggle(at(0), 1), at(0, false));
});

test("a live action and a page read of the same action keep one copy, in time order", () => {
  const a = action({ index: 0, atMs: 10 });
  const b = action({ index: 1, atMs: 20 });
  const c = action({ batch: "b2", index: 0, atMs: 15 });
  let actions = withAction([], b);
  actions = withAction(actions, a);
  actions = withAction(actions, b);
  actions = withAction(actions, c);
  assert.deepEqual(actions.map((x) => x.atMs), [10, 15, 20]);
  // A page that starts mid-batch, then the batch's first action live, in the same millisecond.
  const page = [action({ index: 1, atMs: 30, status: "skipped" }), action({ index: 2, atMs: 30, status: "skipped" })];
  assert.deepEqual(withAction(page, action({ index: 0, atMs: 30, status: "failed" })).map((x) => x.index), [0, 1, 2]);
});
