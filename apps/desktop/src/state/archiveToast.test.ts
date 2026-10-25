import assert from "node:assert/strict";
import { afterEach, beforeEach, test } from "node:test";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";

import type { Conversation, Request, ThreadView, WorkerRow } from "@/ipc/generated";
import { archiveAll } from "@/state/actions";
import { useCliThread } from "@/state/cliThread";
import { useApp } from "@/state/store";
import { useToasts } from "@/state/toasts";

const conversation = (id: string, kind: "session" | "chat", title: string) =>
  ({ id, kind, title, projectId: null, lifecycle: "idle" }) as unknown as Conversation;
const unmerged = (title: string) => ({ id: title, title, ahead: 1, merged: false, diff: null }) as unknown as WorkerRow;
const texts = () => useToasts.getState().toasts.map((entry) => entry.text);
const originals = Object.fromEntries(["window", "document"].map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));

beforeEach(() => {
  Object.defineProperty(globalThis, "window", { configurable: true, value: {} });
  Object.defineProperty(globalThis, "document", { configurable: true, value: { activeElement: null } });
  useToasts.setState({ toasts: [] });
  mockIPC((_command, payload) => {
    const req = (payload as { request: Request }).request;
    if (req.method === "archive") {
      const { conversations } = useApp.getState();
      return { method: req.method, outcomes: req.ids.map((id) => ({ id, conversation: conversations[id], error: null })) };
    }
    return { method: req.method };
  });
});

afterEach(() => {
  clearMocks();
  for (const [key, original] of Object.entries(originals)) {
    if (original) Object.defineProperty(globalThis, key, original);
    else Reflect.deleteProperty(globalThis, key);
  }
});

test("the archive toast names only what was archived, never its workers' titles", async () => {
  useApp.setState({
    conversations: {
      one: conversation("one", "session", "Please start the nightly build from the main branch"),
      two: conversation("two", "session", "Direct messages"),
      three: conversation("three", "session", "Toast"),
      chat: conversation("chat", "chat", "Trading"),
    },
  });
  const workers = [unmerged("Reconcile navigation with latest"), unmerged("Implement direct worker messaging")];
  useCliThread.setState({ views: { one: { workers } as unknown as ThreadView, two: { workers } as unknown as ThreadView } });

  await archiveAll(["one"]);
  assert.deepEqual(texts(), ["Archived session"]);
  assert.deepEqual(useToasts.getState().toasts[0]?.actions.map((action) => action.label), ["View", "Undo"]);

  useToasts.setState({ toasts: [] });
  await archiveAll(["two", "three"]);
  assert.deepEqual(texts(), ["Archived 2 sessions"]);

  useToasts.setState({ toasts: [] });
  await archiveAll(["chat"]);
  assert.deepEqual(texts(), ["Archived chat"]);
});
