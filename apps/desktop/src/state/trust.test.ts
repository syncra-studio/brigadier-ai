import assert from "node:assert/strict";
import { afterEach, beforeEach, test } from "node:test";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";

import type { RequestOf } from "@/ipc/client";
import type { Conversation, FolderTrust, Project } from "@/ipc/generated";
import { type AppState, useApp } from "@/state/store";
import { useToasts } from "@/state/toasts";
import { folderTrust, setFolderTrust, toAsk, trustFailureText } from "@/state/trust";

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

/** The state with these projects and the user at `selection`; `s1` is a session in `a`. */
function at(selection: AppState["selection"], ...projects: Project[]) {
  return {
    selection,
    projects: Object.fromEntries(projects.map((p) => [p.id, p])),
    conversations: { s1: { id: "s1", projectId: "a" } as Conversation },
  };
}

test("only the project the user is in is asked about, in its new session or a chat in it", () => {
  const a = project("a", 2, "/code/a");
  const b = project("b", 1, "/code/b");
  assert.equal(toAsk(at({ type: "draft", kind: "session", projectId: "a" }, a, b))?.id, "a");
  assert.equal(toAsk(at({ type: "conversation", id: "s1" }, a, b))?.id, "a");
  // Many unanswered projects ask nothing while the user is elsewhere.
  assert.equal(toAsk(at({ type: "draft", kind: "chat" }, a, b)), null);
  assert.equal(toAsk(at({ type: "none" }, a, b)), null);
  assert.equal(toAsk(at({ type: "conversation", id: "gone" }, a, b)), null);
});

test("a project is asked about once: not when its folder has an answer, nor when it has no folder", () => {
  const answered = project("a", 1, "/code/a", [answer("/code/a", false)]);
  assert.equal(toAsk(at({ type: "conversation", id: "s1" }, answered)), null);
  assert.equal(toAsk(at({ type: "draft", kind: "session", projectId: "c" }, project("c", 1, null))), null);
});

test("a project given another folder is asked about again", () => {
  const moved = project("a", 1, "/code/new", [answer("/code/old", true)]);
  assert.equal(folderTrust(moved, "/code/old"), true);
  assert.equal(folderTrust(moved, "/code/new"), null);
  assert.equal(toAsk(at({ type: "draft", kind: "session", projectId: "a" }, moved))?.id, "a");
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
  assert.equal(toAsk({ ...useApp.getState(), selection: { type: "draft", kind: "session", projectId: "p" } }), null);
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
  assert.equal(toAsk({ ...useApp.getState(), selection: { type: "draft", kind: "session", projectId: "p" } })?.id, "p");
});
