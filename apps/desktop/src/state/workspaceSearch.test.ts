import assert from "node:assert/strict";
import { afterEach, beforeEach, test } from "node:test";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import type { Request } from "@/ipc/generated";
import { CACHE_BYTES } from "@/state/workspaceCache";
import { dismissSearchFile, EMPTY_FORM, emitSearchUpdate, kept, replaceResults, runSearch, stopSearch, useWorkspaceSearch } from "./workspaceSearch";

const target = { type: "project", projectId: "search-unit" } as const;
const query = { text: "hit", matchCase: false, wholeWord: false, regex: false, include: "", exclude: "" };
const results = (size: number) => ({ checkout: "x", query, files: [{ path: "a.txt", hash: "h", matches: 1,
  lines: [{ line: 1, preview: "x".repeat(size), ranges: [] }] }], done: { files: 1, matches: 1, capped: false }, error: null });
const current = () => useWorkspaceSearch.getState().entries["project:search-unit"]!.results!;
const originalWindow = Object.getOwnPropertyDescriptor(globalThis, "window");
beforeEach(() => {
  Object.defineProperty(globalThis, "window", { configurable: true, value: {} });
  useWorkspaceSearch.setState({ entries: {}, order: [] });
});
afterEach(() => {
  stopSearch(target);
  clearMocks();
  if (originalWindow) Object.defineProperty(globalThis, "window", originalWindow);
  else Reflect.deleteProperty(globalThis, "window");
});

test("closing Search before its id arrives cancels it when the answer arrives", async () => {
  const asked: Request[] = [];
  let answer!: (value: unknown) => void;
  mockIPC((command, args) => {
    if (command !== "ipc_request") return null;
    const request = (args as { request: Request }).request;
    asked.push(request);
    if (request.method === "startSearch") return new Promise((resolve) => { answer = resolve; });
    return { method: request.method };
  });
  runSearch(target, query);
  await new Promise((resolve) => setImmediate(resolve));
  emitSearchUpdate(42, { type: "files", files: results(1).files });
  stopSearch(target);
  answer({ method: "startSearch", id: 42 });
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(asked.at(-1), { method: "cancelSearch", id: 42 });
  assert.deepEqual(useWorkspaceSearch.getState().entries["project:search-unit"]?.results?.files, []);
});

test("search cache evicts older payloads by bytes while keeping their fields", () => {
  let state = kept({ entries: {}, order: [] }, "a", { form: { ...EMPTY_FORM, text: "saved" }, results: results(300) }, 4, 1200);
  state = kept(state, "b", { results: results(300) }, 4, 1200);
  assert.equal(state.entries.a?.results, null);
  assert.equal(state.entries.a?.form.text, "saved");
  assert.ok(state.entries.b?.results);
});

test("oversized results live only until the active Search closes, even after Done", () => {
  useWorkspaceSearch.setState(kept({ entries: {}, order: [] }, "project:search-unit", { results: results(CACHE_BYTES) }));
  stopSearch(target);
  assert.equal(useWorkspaceSearch.getState().entries["project:search-unit"]?.results, null);
});


test("dismissals filter late batches and replacements, survive refresh, and reset for Enter or a new query", async () => {
  const asked: Request[] = [];
  let id = 0;
  mockIPC((command, args) => {
    if (command !== "ipc_request") return null;
    const req = (args as { request: Request }).request;
    asked.push(req);
    if (req.method === "startSearch") return { method: req.method, id: ++id };
    if (req.method === "replaceInFiles") return { method: req.method, outcome: { files: 1, matches: 1, skipped: [], unrestored: [] } };
    return { method: req.method };
  });
  const a = results(1).files[0]!;
  const b = { ...a, path: "b.txt", hash: "b-hash" };
  const start = async (reset = false, next = query) => {
    runSearch(target, next, reset);
    await new Promise((resolve) => setImmediate(resolve));
  };
  const finish = () => emitSearchUpdate(id, { type: "done", files: 2, matches: 2, capped: false });
  await start();
  emitSearchUpdate(id, { type: "files", files: [a] });
  dismissSearchFile(target, a.path);
  emitSearchUpdate(id, { type: "files", files: [a, b] });
  finish();
  assert.deepEqual(current().files, [b]);
  assert.equal(current().files.reduce((sum, file) => sum + file.matches, 0), 1);
  await replaceResults(target, current(), "new", b.path);
  await start();
  emitSearchUpdate(id, { type: "files", files: [a, b] });
  finish();
  await replaceResults(target, current(), "new");
  for (const replacement of asked.filter((req) => req.method === "replaceInFiles")) {
    assert.deepEqual(replacement.files, [{ path: b.path, hash: b.hash, matches: b.matches }]);
  }
  await start(true);
  emitSearchUpdate(id, { type: "files", files: [a, b] });
  finish();
  assert.deepEqual(current().files, [a, b]);
  // Dismiss the old visible payload before the first fresh batch arrives.
  await start();
  dismissSearchFile(target, a.path);
  emitSearchUpdate(id, { type: "files", files: [a, b] });
  finish();
  assert.deepEqual(current().files, [b]);
  await start(false, { ...query, matchCase: true });
  emitSearchUpdate(id, { type: "files", files: [a, b] });
  finish();
  assert.deepEqual(current().files, [a, b]);
});


test("dismissals survive byte-budget and close eviction independently of the result payload", async () => {
  let id = 0;
  mockIPC((command, args) => {
    if (command !== "ipc_request") return null;
    const req = (args as { request: Request }).request;
    return req.method === "startSearch" ? { method: req.method, id: ++id } : { method: req.method };
  });
  const a = results(1).files[0]!;
  const b = { ...results(CACHE_BYTES).files[0]!, path: "big.txt" };
  const start = async (reset = false, next = query) => {
    runSearch(target, next, reset);
    await new Promise((resolve) => setImmediate(resolve));
    emitSearchUpdate(id, { type: "files", files: [a, b] });
    emitSearchUpdate(id, { type: "done", files: 2, matches: 2, capped: false });
  };
  await start();
  dismissSearchFile(target, a.path);
  stopSearch(target);
  assert.equal(current(), null, "closing evicts the oversized remaining payload");
  await start();
  assert.deepEqual(current().files.map((file) => file.path), [b.path]);
  assert.equal(current().filteredFiles, 1);
  useWorkspaceSearch.setState(kept(useWorkspaceSearch.getState(), "other", { results: results(300) }, 4, 1200));
  assert.equal(current(), null, "the byte budget evicts the old payload");
  await start();
  assert.deepEqual(current().files.map((file) => file.path), [b.path]);
  assert.equal(current().filteredFiles, 1);
  stopSearch(target);
  await start(true);
  assert.deepEqual(current().files.map((file) => file.path), [a.path, b.path], "Enter resets even after eviction");
  dismissSearchFile(target, a.path);
  stopSearch(target);
  await start(false, { ...query, include: "*.txt" });
  assert.deepEqual(current().files.map((file) => file.path), [a.path, b.path], "changed options reset after eviction");
});
