import assert from "node:assert/strict";
import { test } from "node:test";

import { threadEntries } from "@/components/transcript/activity";
import { TranscriptFolder } from "@/components/transcript/transcript";
import type { ProviderEvent, RawEntry } from "@/ipc/generated";

function entry(streamSeq: number, atMs: number, event: ProviderEvent): RawEntry {
  return { streamSeq, atMs, event };
}

test("a worker streams reasoning, settles it before an action, then streams the next summary", () => {
  const folder = new TranscriptFolder();
  const first = entry(1, 1000, { type: "reasoningDelta", itemId: "r1", text: "Read the settings" });
  folder.push([first]);
  let items = folder.push([first, entry(2, 4000, { type: "reasoningDelta", itemId: "r1", text: " first." })]).items;
  assert.deepEqual(items[0], { kind: "reasoning", key: "reasoning:r1", text: "Read the settings first.",
    streaming: true, startedAtMs: 1000, endedAtMs: 4000 });
  items = folder.push([
    entry(3, 6000, { type: "toolCall", itemId: "read", name: "Read", input: "settings.ts", output: null, status: "inProgress" }),
    entry(4, 7000, { type: "reasoning", itemId: "r1", text: "Read settings before changing the theme." }),
    entry(5, 8000, { type: "reasoningDelta", itemId: "r2", text: "Use the existing store." }),
  ]).items;
  assert.deepEqual(threadEntries(items).map((item) => item.kind === "actions" ? "action" : item.item.kind),
    ["reasoning", "action", "reasoning"]);
  assert.equal(items[0]!.kind === "reasoning" && items[0].streaming, false);
  assert.equal(items[0]!.kind === "reasoning" && items[0].startedAtMs, 1000);
  assert.equal(items[2]!.kind === "reasoning" && items[2].streaming, true);
  items = folder.push([entry(6, 9000, { type: "turnCompleted", turnId: "turn", status: "interrupted", durationMs: 8000, usage: null })]).items;
  assert.equal(items[2]!.kind === "reasoning" && items[2].streaming, false);
});

test("updates from an earlier action do not stop the next live summary", () => {
  const folder = new TranscriptFolder();
  const command: ProviderEvent = { type: "command", itemId: "cmd", command: "pnpm test", cwd: null,
    status: "inProgress", exitCode: null, output: null, durationMs: null };
  folder.push([entry(1, 0, command), entry(2, 1000, { type: "reasoningDelta", itemId: "r", text: "Check " })]);
  const items = folder.push([
    entry(3, 2000, { type: "commandOutputDelta", itemId: "cmd", text: "Test output" }),
    entry(4, 2500, { type: "contextSize", usedTokens: 100, windowTokens: 1000 }),
    entry(5, 3000, { type: "reasoningDelta", itemId: "r", text: "the result." }),
  ]).items;
  assert.equal(items[1]!.kind === "reasoning" && items[1].streaming, true);
  assert.equal(items[1]!.kind === "reasoning" && items[1].text, "Check the result.");
});
