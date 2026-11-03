import assert from "node:assert/strict";
import { test } from "node:test";

import { REFRESHED, withSeams } from "@/app/side-panel/workerSeams";
import type { FileEntry, ProviderEvent, ThreadView } from "@/ipc/generated";

const at = (atMs: number, event: ProviderEvent): FileEntry => ({ atMs, event }) as FileEntry;
const user = (itemId: string, text: string, ms: number) => at(ms, { type: "message", itemId, role: "user", text });
const said = (itemId: string, text: string, ms: number) => at(ms, { type: "message", itemId, role: "assistant", text });
const compacted = (ms: number) =>
  at(ms, { type: "compactionEnded", automatic: true, tokensBefore: 150_000, tokensAfter: 20_000, error: null } as ProviderEvent);
const shown = (view: ThreadView) =>
  withSeams(view).map(({ event }) => (event.type === "notice" ? `— ${event.message}` : event.type === "message" ? `${event.role}: ${event.text}` : event.type));

test("a worker's panel shows each compaction once and no mark of a rebirth, its note and fix turns under the hood (core-goals D193)", () => {
  const view: ThreadView = {
    entries: [
      user("b", "Brief: grow.", 1),
      said("a1", "Grown.", 2),
      compacted(3),
      user("n1", "[Brigadier] A fresh session will continue this work. Finish the step…", 4),
      said("a2", "Handed off.", 5),
      user("f1", "[Brigadier] A fresh session will continue this work. Hand-off check: your hand-off note at /x leaves out…", 6),
      said("a3", "Fixed.", 7),
      user("s1", "Next.", 8),
      said("a4", "On it.", 9),
      user("n2", "[Brigadier] A fresh session will continue this work. Finish…", 10),
      said("a5", "Handed off.", 11),
      user("s2", "Carry on from the hand-off note.", 12),
      said("a6", "Carrying on.", 13),
    ],
    turns: [],
    seams: [
      { n: 1, open: false, at: "s1", carried: false, nudge: "n1", note: "/x/1.md", start: null, death: null, movedTo: null },
      { n: 2, open: false, at: "s2", carried: true, nudge: "n2", note: "/x/2.md", start: null, death: null, movedTo: null },
    ],
  };
  assert.deepEqual(shown(view), [
    "user: Brief: grow.",
    "assistant: Grown.",
    `— ${REFRESHED}`,
    "user: Next.",
    "assistant: On it.",
    "assistant: Carrying on.",
  ]);
  // While it writes its note its note turn is already under the hood.
  const writing = { ...view, entries: view.entries.slice(0, 5), seams: [{ ...view.seams![0]!, open: true, at: null }] };
  assert.deepEqual(shown(writing), ["user: Brief: grow.", "assistant: Grown.", `— ${REFRESHED}`]);
});
