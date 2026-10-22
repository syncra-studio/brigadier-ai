import assert from "node:assert/strict";
import { test } from "node:test";

import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";

import type { FileEntry, ProviderEvent, ThreadView } from "@/ipc/generated";
import { applyThreadNews, loadThreadView, mergeEntries, useCliThread } from "@/state/cliThread";

const at = (event: ProviderEvent, atMs: number): FileEntry => ({ event, atMs });
const command = (status: "inProgress" | "completed", output: string | null = null): ProviderEvent => ({
  type: "command",
  itemId: "c",
  command: "ls",
  cwd: null,
  status,
  exitCode: status === "completed" ? 0 : null,
  output,
  durationMs: null,
});
const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

test("the stream extends the file item by item, as the engine merges them", () => {
  const user = at({ type: "message", itemId: "u", role: "user", text: "Hi" }, 1);
  const file = [user, at(command("inProgress"), 2)];
  const merged = mergeEntries(file, [
    at({ type: "commandOutputDelta", itemId: "c", text: "one\n" }, 3),
    at({ type: "messageDelta", itemId: "m", text: "Wor" }, 4),
    at({ type: "messageDelta", itemId: "m", text: "king" }, 5),
    at({ type: "error", error: { kind: "other", message: "boom", willRetry: false, limit: null, code: null } }, 6),
  ]);
  assert.deepEqual(merged, [
    user,
    { event: command("inProgress", "one\n"), atMs: 2, updatedAtMs: 3 },
    { event: { type: "messageDelta", itemId: "m", text: "Working" }, atMs: 4, updatedAtMs: 5 },
    at({ type: "error", error: { kind: "other", message: "boom", willRetry: false, limit: null, code: null } }, 6),
  ]);
  // What didn't change keeps its identity: only the rows that did draw again.
  assert.equal(merged[0], user);
  // A call's end takes its start's place, keeping what only its start said, and when it began.
  const end = command("completed", "one\ntwo\n");
  const ended = mergeEntries(merged, [{ event: end.type === "command" ? { ...end, command: "" } : end, atMs: 7 }]);
  assert.deepEqual(ended[1], { event: command("completed", "one\ntwo\n"), atMs: 2, updatedAtMs: 7 });
  assert.equal(ended.length, 4);
});

test("a thread is read once, follows the stream, and is read again when its turn ends", async () => {
  Object.defineProperty(globalThis, "window", { configurable: true, value: { crypto: globalThis.crypto } });
  const first: ThreadView = {
    entries: [at({ type: "message", itemId: "u1", role: "user", text: "Go" }, 1)],
    turns: [{ userMessage: "u1", startedAtMs: 1, endedAtMs: null, end: null, error: null }],
  };
  const reads: string[] = [];
  let answer: ThreadView = first;
  // A read held until the test lets it land.
  let held = false;
  let land: (() => void) | null = null;
  mockIPC((cmd, payload) => {
    if (cmd !== "ipc_request") return null;
    const { request } = payload as { request: { method: string; conversationId: string } };
    reads.push(request.conversationId);
    const response = { method: "threadView", view: answer };
    if (!held) return response;
    return new Promise((resolve) => {
      land = () => resolve(response);
    });
  });
  try {
    await loadThreadView("conv");
    assert.deepEqual(useCliThread.getState().views["conv"], first);

    // Deltas as they come, one state update for those that came together.
    applyThreadNews({ conversationId: "conv", entries: [at({ type: "messageDelta", itemId: "m", text: "Do" }, 2)] });
    applyThreadNews({ conversationId: "conv", entries: [at({ type: "messageDelta", itemId: "m", text: "ne" }, 3)] });
    // Another conversation's, not read yet, waits for it to open.
    applyThreadNews({ conversationId: "other", entries: [at({ type: "messageDelta", itemId: "x", text: "…" }, 3)] });
    await settle();
    assert.deepEqual(useCliThread.getState().views["conv"]?.entries.at(-1), { event: { type: "messageDelta", itemId: "m", text: "Done" }, atMs: 2, updatedAtMs: 3 });
    assert.equal(useCliThread.getState().views["other"], undefined);

    // The turn ended: read again, and what the stream says meanwhile is added once the read lands,
    // whole items only (a delta may be in the read already).
    const done: ThreadView = {
      entries: [first.entries[0] as FileEntry, at({ type: "message", itemId: "m", role: "assistant", text: "Done" }, 2)],
      turns: [{ userMessage: "u1", startedAtMs: 1, endedAtMs: 4, end: "done", error: null }],
    };
    answer = done;
    held = true;
    applyThreadNews({ conversationId: "conv", entries: [], turnEnded: true });
    await settle();
    applyThreadNews({ conversationId: "conv", entries: [at({ type: "message", itemId: "u2", role: "user", text: "Next" }, 5)] });
    applyThreadNews({ conversationId: "conv", entries: [at({ type: "messageDelta", itemId: "m2", text: "Sure" }, 6)] });
    await settle();
    held = false;
    (land as (() => void) | null)?.();
    await settle();
    assert.deepEqual(reads, ["conv", "conv"]);
    const view = useCliThread.getState().views["conv"];
    assert.deepEqual(view?.turns, done.turns);
    assert.deepEqual(view?.entries.map((entry) => (entry.event.type === "message" ? entry.event.text : entry.event.type)), ["Go", "Done", "Next"]);
  } finally {
    clearMocks();
    useCliThread.setState({ views: {} });
  }
});
