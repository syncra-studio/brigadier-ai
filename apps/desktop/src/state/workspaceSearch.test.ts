import assert from "node:assert/strict";
import { afterEach, beforeEach, test } from "node:test";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import type { Request } from "@/ipc/generated";
import { CACHE_BYTES } from "@/state/workspaceCache";
import { EMPTY_FORM, emitSearchUpdate, kept, runSearch, stopSearch, useWorkspaceSearch } from "./workspaceSearch";

const target = { type: "project", projectId: "search-unit" } as const;
const query = { text: "hit", matchCase: false, wholeWord: false, regex: false, include: "", exclude: "" };
const results = (size: number) => ({ checkout: "x", query, files: [{ path: "a.txt", hash: "h", matches: 1,
  lines: [{ line: 1, preview: "x".repeat(size), ranges: [] }] }], done: { files: 1, matches: 1, capped: false }, error: null });
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
