import assert from "node:assert/strict";
import { afterEach, beforeEach, test } from "node:test";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";

import type { RequestOf } from "@/ipc/client";
import type { FolderTrust, Project } from "@/ipc/generated";
import { useApp } from "@/state/store";
import { useToasts } from "@/state/toasts";
import { folderTrust, nextToAsk, setFolderTrust, trustFailureText } from "@/state/trust";

function project(id: string, createdAtMs: number, path: string | null, trust: FolderTrust[] = []): Project {
  return {
    id,
    name: id,
    createdAtMs,
    repos: path === null ? [] : [{ path, name: id }],
    prefs: {} as Project["prefs"],
    trust,
  };
}

const answer = (path: string, trusted: boolean): FolderTrust => ({ path, trusted, decidedAtMs: 1 });

test("the oldest project whose folder has no answer is asked about next", () => {
  const older = project("older", 1, "/code/older");
  const newer = project("newer", 2, "/code/newer");
  assert.equal(nextToAsk([newer, older])?.id, "older");
  assert.equal(nextToAsk([newer, { ...older, trust: [answer("/code/older", false)] }])?.id, "newer");
});

test("no project is asked about when every folder has an answer, or a project has no folder", () => {
  assert.equal(
    nextToAsk([
      project("a", 1, "/code/a", [answer("/code/a", true)]),
      project("b", 2, "/code/b", [answer("/code/b", false)]),
      project("c", 0, null),
    ]),
    null,
  );
  assert.equal(nextToAsk([]), null);
});

test("a project given another folder is asked about again", () => {
  const moved = project("moved", 1, "/code/new", [answer("/code/old", true)]);
  assert.equal(folderTrust(moved, "/code/old"), true);
  assert.equal(folderTrust(moved, "/code/new"), null);
  assert.equal(nextToAsk([moved])?.id, "moved");
});

test("a failure names the agent that may ask again, in plain words", () => {
  assert.equal(
    trustFailureText("Codex: Codex's config.toml can't be edited safely: bad table", true),
    "Saved. Brigadier couldn't record it for Codex, so Codex may ask again in its terminal: Codex's config.toml can't be edited safely: bad table",
  );
  assert.match(trustFailureText("disk full", true), /^Saved\. .*: disk full$/);
  assert.match(trustFailureText("Claude: locked", false), /^Saved\. .*: Claude: locked$/);
});

const originalWindow = Object.getOwnPropertyDescriptor(globalThis, "window");
let sent: RequestOf<"setFolderTrust">[];
let failures: string[];

beforeEach(() => {
  Object.defineProperty(globalThis, "window", { configurable: true, value: {} });
  sent = [];
  failures = [];
  useToasts.setState({ toasts: [] });
  useApp.setState({ projects: { p: project("p", 1, "/code/p") } });
  mockIPC((command, payload) => {
    assert.equal(command, "ipc_request");
    const req = (payload as { request: RequestOf<"setFolderTrust"> }).request;
    assert.equal(req.method, "setFolderTrust");
    sent.push(req);
    const stored = project("p", 1, "/code/p", [answer(req.path ?? "/code/p", req.trusted)]);
    return { method: req.method, report: { project: stored, failures } };
  });
});

afterEach(() => {
  clearMocks();
  if (originalWindow) Object.defineProperty(globalThis, "window", originalWindow);
  else Reflect.deleteProperty(globalThis, "window");
});

test("answering records the folder's trust and stores the project, so it is not asked again", async () => {
  await setFolderTrust("p", "/code/p", false);
  assert.deepEqual(sent, [{ method: "setFolderTrust", id: "p", path: "/code/p", trusted: false }]);
  assert.equal(folderTrust(useApp.getState().projects.p, "/code/p"), false);
  assert.equal(nextToAsk(Object.values(useApp.getState().projects)), null);
  assert.deepEqual(useToasts.getState().toasts, []);
});

test("what couldn't be written in an agent's settings is shown, and the answer still stands", async () => {
  failures = ["Codex: locked"];
  await setFolderTrust("p", "/code/p", true);
  assert.equal(folderTrust(useApp.getState().projects.p, "/code/p"), true);
  assert.deepEqual(
    useToasts.getState().toasts.map((entry) => [entry.tone, entry.text]),
    [["error", "Saved. Brigadier couldn't record it for Codex, so Codex may ask again in its terminal: locked"]],
  );
});

test("a failed request leaves the folder unanswered, so it is still asked about", async () => {
  mockIPC(() => {
    throw new Error("daemon unreachable");
  });
  await assert.rejects(setFolderTrust("p", "/code/p", true), /daemon unreachable/);
  assert.equal(nextToAsk(Object.values(useApp.getState().projects))?.id, "p");
});
