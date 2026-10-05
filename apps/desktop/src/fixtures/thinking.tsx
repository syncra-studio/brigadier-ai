/** Standalone verification entry: the real thread layout, with interleaved reasoning. */
import { leadTranscript } from "@/fixtures/flow";

import type { RawEntry, ThinkingSegment } from "@/ipc/generated";
import { useBoard } from "@/state/board";

const query = new URLSearchParams(location.search);
const done = query.get("view") === "done";
const now = Date.now();
const segment = (itemId: string, position: number, text: string, start: number, end: number, complete: boolean): ThinkingSegment => ({
  itemId, position, throughPosition: position, requestId: "r2", text,
  startedAtMs: now - start * 1000, updatedAtMs: now - end * 1000, complete,
});
const first = "I’ll check how the app stores settings before adding the theme choice. The existing store should keep the choice across restarts.";
const second = "The settings store already loads before the first paint. I’m checking the remaining tests so the theme applies as soon as the user changes it.";
useBoard.setState(({ board }) => {
  if (!board) return {};
  const transcript = board.transcripts["t2"]!;
  const raw: RawEntry[] = [
    { streamSeq: 0, atMs: now - 114000, event: { type: "reasoningDelta", itemId: "thought-read", text: first } },
    ...transcript.entries,
    { streamSeq: 100, atMs: now - 7000, event: { type: "reasoningDelta", itemId: "thought-test", text: second } },
    ...(done ? [{ streamSeq: 101, atMs: now - 2000, event: { type: "reasoning", itemId: "thought-test", text: second } } as RawEntry] : []),
  ];
  leadTranscript.splice(0, leadTranscript.length, ...raw);
  return { board: { ...board, run: done ? "idle" : "running", runRequest: done ? null : "r2",
    thinking: [segment("thought-read", 11.5, first, done ? 1253 : 126, done ? 1245 : 118, true),
      segment("thought-test", 25, second, done ? 24 : 7, done ? 17 : 0, done)],
    transcripts: { ...board.transcripts, t2: { ...transcript, entries: raw } } } };
});

