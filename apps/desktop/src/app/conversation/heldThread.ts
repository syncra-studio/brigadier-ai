import type { Block, ThreadTree } from "@/app/conversation/blocks";
import type { ThreadHolding } from "@/components/conversation";

/*
 * What a long thread costs per streamed word stays flat (#627, #628): assistant-ui holds only the
 * turns its list asks for (`heldRows`), and a block built again the same as last drawn is the one
 * drawn (`keepDrawnBlocks`), so what reads blocks by identity does the work for the changed alone.
 */

/** Conversations whose blocks as last drawn are kept while others open. */
const KEPT_CONVERSATIONS = 4;

/**
 * Which rows of the branch shown (oldest first) assistant-ui holds: those `hold` asks for (`from`
 * to `to`, and `ids`) and the newest turn, each with its whole turn (a turn starts at a user
 * message), so an answer's parent is always held with it.
 */
export function heldRows(branch: readonly { id: string; kind: "user" | "block" }[], hold: ThreadHolding): Uint8Array {
  const count = branch.length;
  const held = new Uint8Array(count);
  const mark = (from: number, to: number) => held.fill(1, Math.max(0, from), Math.min(count, to + 1));
  mark(hold.from, hold.to);
  if (hold.ids.length > 0) {
    const wanted = new Set(hold.ids);
    branch.forEach((row, index) => {
      if (wanted.has(row.id)) mark(index, index);
    });
  }
  let newest = count - 1;
  while (newest > 0 && branch[newest]?.kind !== "user") newest--;
  mark(newest, count - 1);
  for (let start = 0; start < count; ) {
    let end = start + 1;
    while (end < count && branch[end]?.kind !== "user") end++;
    if (held.subarray(start, end).includes(1)) held.fill(1, start, end);
    start = end;
  }
  return held;
}

/** Each conversation's blocks as last drawn, by key, for the last few opened. */
const drawnBlocks = new Map<string, Map<string, Block>>();

/**
 * The thread with each block built again the same as last drawn being the one drawn: its turns
 * are built afresh on every streamed word, but only the streaming one changed, so what reads
 * them by identity (the items, the list) does the work for that one alone.
 */
export function keepDrawnBlocks(conversationId: string, tree: ThreadTree): ThreadTree {
  const before = drawnBlocks.get(conversationId);
  drawnBlocks.delete(conversationId);
  const now = new Map<string, Block>();
  const kept = new Map<Block, Block>();
  for (const node of tree.nodes) {
    let block = kept.get(node.block);
    if (!block) {
      const last = before?.get(node.block.key);
      block = last && sameData(last, node.block) ? last : node.block;
      kept.set(node.block, block);
      now.set(block.key, block);
    }
    node.block = block;
  }
  drawnBlocks.set(conversationId, now);
  for (const key of drawnBlocks.keys()) if (drawnBlocks.size > KEPT_CONVERSATIONS) drawnBlocks.delete(key);
  return tree;
}

/** Whether two values of plain data (what a block is made of) are the same, part by part. */
export function sameData(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (typeof a !== "object" || typeof b !== "object" || a === null || b === null) return false;
  if (Array.isArray(a)) {
    if (!Array.isArray(b) || a.length !== b.length) return false;
    for (let index = 0; index < a.length; index++) if (!sameData(a[index], b[index])) return false;
    return true;
  }
  if (Array.isArray(b)) return false;
  const keys = Object.keys(a);
  if (keys.length !== Object.keys(b).length) return false;
  for (const key of keys) {
    if (!sameData((a as Record<string, unknown>)[key], (b as Record<string, unknown>)[key])) return false;
  }
  return true;
}
