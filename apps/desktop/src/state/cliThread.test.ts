import assert from "node:assert/strict";
import { test } from "node:test";

import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";

import { turnCommands } from "@/app/conversation/turnCommands";
import type { BackgroundTask, FileEntry, ProviderEvent, ThreadView } from "@/ipc/generated";
import { applyThreadNews, loadThreadView, mergeEntries, resyncThreadViews, unreadSince, useCliThread } from "@/state/cliThread";

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
    turns: [{ userMessage: "u1", startedAtMs: 1, endedAtMs: null, end: null, error: null, requests: [] }],
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
      turns: [{ userMessage: "u1", startedAtMs: 1, endedAtMs: 4, end: "done", error: null, requests: [] }],
    };
    answer = done;
    held = true;
    applyThreadNews({ conversationId: "conv", entries: [], ended: "done" });
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

test("in a page, what streamed between two frames is drawn once, at the frame", async () => {
  const page = globalThis as unknown as { document?: unknown; requestAnimationFrame?: unknown; cancelAnimationFrame?: unknown };
  const frames: (() => void)[] = [];
  page.document = { hidden: false };
  page.requestAnimationFrame = (draw: () => void) => frames.push(draw);
  page.cancelAnimationFrame = () => {};
  const view: ThreadView = { entries: [at({ type: "message", itemId: "u", role: "user", text: "Go" }, 1)], turns: [] };
  useCliThread.setState({ views: { framed: view } });
  let updates = 0;
  const stop = useCliThread.subscribe(() => (updates += 1));
  try {
    // Each delta its own task, as the WebView hands them over.
    for (const [index, text] of ["One", " two", " three"].entries()) {
      applyThreadNews({ conversationId: "framed", entries: [at({ type: "messageDelta", itemId: "m", text }, 2 + index)] });
      await settle();
    }
    assert.equal(updates, 0);
    assert.equal(frames.length, 1);
    frames[0]?.();
    assert.equal(updates, 1);
    assert.deepEqual(useCliThread.getState().views["framed"]?.entries.at(-1)?.event, { type: "messageDelta", itemId: "m", text: "One two three" });
  } finally {
    stop();
    delete page.document;
    delete page.requestAnimationFrame;
    delete page.cancelAnimationFrame;
  }
});

test("a delta waiting for the frame when a read of its thread lands shows once", async () => {
  Object.defineProperty(globalThis, "window", { configurable: true, value: { crypto: globalThis.crypto } });
  const page = globalThis as unknown as { document?: unknown; requestAnimationFrame?: unknown; cancelAnimationFrame?: unknown };
  const frames: (() => void)[] = [];
  page.document = { hidden: false };
  page.requestAnimationFrame = (draw: () => void) => frames.push(draw);
  page.cancelAnimationFrame = () => {};
  const user = at({ type: "message", itemId: "u", role: "user", text: "Go" }, 1);
  useCliThread.setState({ views: { reread: { entries: [user], turns: [] } } });
  // The read's snapshot already holds the delta's words.
  const read: ThreadView = { entries: [user, at({ type: "messageDelta", itemId: "m", text: "hello" }, 2)], turns: [] };
  let land: (() => void) | null = null;
  mockIPC((cmd) => (cmd === "ipc_request" ? new Promise((resolve) => (land = () => resolve({ method: "threadView", view: read }))) : null));
  try {
    const reading = loadThreadView("reread");
    await settle();
    // The delta comes while the read is under way, and waits for the next frame.
    applyThreadNews({ conversationId: "reread", entries: [at({ type: "messageDelta", itemId: "m", text: "hello" }, 2)] });
    // The read lands before that frame.
    (land as (() => void) | null)?.();
    await reading;
    for (const draw of frames.splice(0)) draw();
    assert.deepEqual(useCliThread.getState().views["reread"]?.entries.at(-1)?.event, { type: "messageDelta", itemId: "m", text: "hello" });
  } finally {
    clearMocks();
    useCliThread.setState({ views: {} });
    delete page.document;
    delete page.requestAnimationFrame;
    delete page.cancelAnimationFrame;
  }
});

/** Whether the block of the turn started at 1000 reads as finished: its turn over, nothing holding it. */
const finished = (view: ThreadView | undefined) => !!view && view.turns.at(-1)?.end !== null && turnCommands(view, view.turns, [1000]) === 0;

test("a background command's end and the turn it starts land together: the block it held is never seen finished between", async () => {
  Object.defineProperty(globalThis, "window", { configurable: true, value: { crypto: globalThis.crypto } });
  const call: FileEntry = at({ type: "command", itemId: "toolu_watch", command: "gh run watch 12", cwd: null, status: "completed", exitCode: 0, output: null, durationMs: null }, 1200);
  const watch: BackgroundTask = {
    id: "bwatch", itemId: "toolu_watch", kind: "shell", label: "gh run watch 12", status: "completed", startedAtMs: 1250, endedAtMs: 3000, outputFile: null, summary: null, lastLine: null,
  };
  const user = at({ type: "message", itemId: "u1", role: "user", text: "Watch CI" }, 1000);
  // The turn ended while the command ran; the command ended (exit 0) with no turn open, and its
  // end owes the turn the CLI starts for it (D21): the block stays held, its work open.
  const ended: ThreadView = {
    entries: [user, call],
    turns: [{ userMessage: "u1", startedAtMs: 1000, endedAtMs: 2000, end: "done", error: null, requests: [] }],
    background: [watch],
    jobs: { waking: true },
  };
  // The CLI's turn for it began: the engine's thread reads that turn as open again.
  const reopened: ThreadView = { ...ended, turns: [{ ...ended.turns[0]!, endedAtMs: null, end: null }], jobs: {} };
  let land: (() => void) | null = null;
  let answer = ended;
  let held = false;
  mockIPC((cmd) => {
    if (cmd !== "ipc_request") return null;
    const response = { method: "threadView", view: answer };
    return held ? new Promise((resolve) => (land = () => resolve(response))) : response;
  });
  const seen: boolean[] = [];
  const unsubscribe = useCliThread.subscribe((state) => seen.push(finished(state.views["wake"])));
  try {
    await loadThreadView("wake");
    assert.equal(finished(useCliThread.getState().views["wake"]), false);
    // The engine says, in one piece of news, that the turn opened and that nothing is owed now.
    answer = reopened;
    held = true;
    applyThreadNews({ conversationId: "wake", entries: [], opened: true, jobs: {} });
    await settle();
    assert.equal(finished(useCliThread.getState().views["wake"]), false, "the hold is gone before the turn it owed shows open");
    (land as (() => void) | null)?.();
    await settle();
    assert.deepEqual(useCliThread.getState().views["wake"]?.turns, reopened.turns);
    assert.deepEqual(useCliThread.getState().views["wake"]?.jobs, {});

    // The same while a read is already under way (the command's own end's): that read predates
    // the turn, so the news waits for the read after it.
    useCliThread.setState({ views: { wake: ended } });
    answer = ended;
    land = null;
    const reading = loadThreadView("wake");
    await settle();
    const first = land as (() => void) | null;
    applyThreadNews({ conversationId: "wake", entries: [], opened: true, jobs: {} });
    await settle();
    answer = reopened;
    first?.();
    await reading;
    await settle();
    assert.equal(finished(useCliThread.getState().views["wake"]), false, "the earlier read lands with the newer hold");
    (land as (() => void) | null)?.();
    await settle();
    assert.deepEqual(useCliThread.getState().views["wake"]?.turns, reopened.turns);
    assert.deepEqual(seen.filter(Boolean), [], "never finished between");
  } finally {
    unsubscribe();
    clearMocks();
    useCliThread.setState({ views: {} });
  }
});

test("a view dropped on reconnect is unread from then on; one never read, since the app started", async () => {
  const empty: ThreadView = { entries: [], turns: [] };
  useCliThread.setState({ views: { shown: empty, away: empty } });
  mockIPC((cmd) => (cmd === "ipc_request" ? { method: "threadView", view: empty } : null));
  try {
    const started = unreadSince("never");
    assert.ok(started <= Date.now());
    const before = Date.now();
    await resyncThreadViews(["shown"]);
    assert.equal(useCliThread.getState().views["away"], undefined);
    // Its earlier turns are no message on its way; one sent from now on is.
    assert.ok(unreadSince("away") >= before && unreadSince("away") <= Date.now());
    assert.equal(unreadSince("shown"), started, "a shown view is kept, not dropped");
    assert.equal(unreadSince("never"), started);
  } finally {
    clearMocks();
    useCliThread.setState({ views: {} });
  }
});
