import assert from "node:assert/strict";
import { test } from "node:test";

import type { Block, ThreadNode } from "@/app/conversation/blocks";
import { heldRows, keepDrawnBlocks, sameData } from "@/app/conversation/heldThread";

/** Turns 1 to `turns`, each a user message and its answer. */
function branch(turns: number): { id: string; kind: "user" | "block" }[] {
  return Array.from({ length: turns }, (_, index) => [
    { id: `u${index + 1}`, kind: "user" as const },
    { id: `r${index + 1}`, kind: "block" as const },
  ]).flat();
}

const heldIds = (rows: ReturnType<typeof branch>, held: Uint8Array) => rows.filter((_, index) => held[index]).map((row) => row.id);

test("assistant-ui holds the rows asked for and the newest turn, each with its whole turn", () => {
  const rows = branch(10);
  // An answer asked for brings its user message; the newest turn comes along.
  assert.deepEqual(heldIds(rows, heldRows(rows, { key: "c", from: 5, to: 5, ids: [] })), ["u3", "r3", "u10", "r10"]);
  // A message asked for by id, far from the range.
  assert.deepEqual(heldIds(rows, heldRows(rows, { key: "c", from: 18, to: 19, ids: ["u1"] })), ["u1", "r1", "u10", "r10"]);
  // Nothing asked for: the newest turn alone.
  assert.deepEqual(heldIds(rows, heldRows(rows, { key: "c", from: 0, to: -1, ids: [] })), ["u10", "r10"]);
});

test("rows before the first user message are a turn of their own", () => {
  const rows = [{ id: "s", kind: "block" as const }, ...branch(2)];
  assert.deepEqual(heldIds(rows, heldRows(rows, { key: "c", from: 0, to: 0, ids: [] })), ["s", "u2", "r2"]);
});

const block = (key: string, text: string) => ({ key, texts: [{ text, position: 1 }], rows: [{ label: "Ran", steps: [1, 2] }] }) as unknown as Block;

function tree(blocks: Block[]): { nodes: ThreadNode[]; headId: string | null } {
  return { nodes: blocks.map((each, index) => ({ id: `request:${each.key}`, parentId: index === 0 ? null : `request:${blocks[index - 1]?.key}`, kind: "block", block: each, head: null })), headId: null };
}

test("a block built again the same as last drawn is the one drawn; a changed one is the new one", () => {
  const first = tree([block("a", "one"), block("b", "two")]);
  keepDrawnBlocks("held-test", first);
  const [a, b] = first.nodes.map((node) => node.block);
  const again = keepDrawnBlocks("held-test", tree([block("a", "one"), block("b", "two, and more")]));
  assert.equal(again.nodes[0]?.block, a);
  assert.notEqual(again.nodes[1]?.block, b);
  assert.equal(again.nodes[1]?.block.texts[0]?.text, "two, and more");
});

test("plain data is the same part by part", () => {
  assert.ok(sameData({ a: [1, { b: "x" }], c: null }, { a: [1, { b: "x" }], c: null }));
  assert.ok(!sameData({ a: [1, 2] }, { a: [1, 2, 3] }));
  assert.ok(!sameData({ a: 1 }, { a: 1, b: undefined }));
  assert.ok(!sameData([1], { 0: 1 }));
});
