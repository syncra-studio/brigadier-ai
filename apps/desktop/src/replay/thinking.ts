import { blockSequence, buildBlocks } from "@/app/conversation/blocks";
import type { EventEnvelope, Message } from "@/ipc/generated";
import { applyToBoard, emptyBoard } from "@/state/board";

/** Sample a recorded stream through the same board reducer and row derivation as the thread. */
export function replayThinking(conversationId: string, events: readonly EventEnvelope[], start: number, end: number) {
  let board = emptyBoard(conversationId);
  const messages: Message[] = [];
  let next = 0;
  const ordered = events.toSorted((a, b) => a.seq - b.seq);
  const frames: { atMs: number; onlyThinking: boolean; liveText: string | null; kinds: string[] }[] = [];
  for (let now = start; now <= end; now += 1000) {
    while (next < ordered.length && ordered[next]!.atMs <= now) {
      const envelope = ordered[next++]!;
      if (envelope.stream !== `conversation:${conversationId}`) continue;
      board = applyToBoard(board, envelope);
      if (envelope.event.type === "messageAppended") messages.push(envelope.event.message);
    }
    const block = buildBlocks(messages, {}, false, board, []).at(-1);
    const rows = block ? blockSequence({ ...block, steers: [] }) : [];
    const live = rows.find((entry) => entry.kind === "thinking" && entry.live);
    frames.push({ atMs: now, onlyThinking: (!block || block.state === "working") && rows.length === 0,
      liveText: live?.kind === "thinking" ? live.segment.text : null, kinds: rows.map((entry) => entry.kind) });
  }
  return frames;
}
