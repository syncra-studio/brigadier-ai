import assert from "node:assert/strict";
import { test } from "node:test";
import { mockIPC } from "@tauri-apps/api/mocks";

import type { Catalog, Conversation, EventEnvelope, Project, Request } from "@/ipc/generated";
const storage = new Map<string, string>();
Object.defineProperty(globalThis, "localStorage", { configurable: true, value: {
  getItem: (key: string) => storage.get(key) ?? null,
  setItem: (key: string, value: string) => storage.set(key, value),
  removeItem: (key: string) => storage.delete(key),
} });
Object.defineProperty(globalThis, "window", { configurable: true, value: { localStorage } });
const { loadCatalog } = await import("./actions");
const { applyEvents, useApp } = await import("./store");

const project = { id: "project", name: "Project", repos: [], createdAtMs: 1 } as unknown as Project;
const session = { id: "session", kind: "session", projectId: project.id, title: "New session", lifecycle: "active", sideOf: null, pinnedAtMs: null, updatedAtMs: 1 } as Conversation;
const event = (value: EventEnvelope["event"]): EventEnvelope => ({ seq: 1, stream: "catalog", streamSeq: 1, atMs: 1, event: value });

function deferredCatalog(catalog: Catalog) {
  let answer!: () => void;
  const waiting = new Promise<{ method: "getCatalog"; catalog: Catalog }>((resolve) => { answer = () => resolve({ method: "getCatalog", catalog }); });
  mockIPC((_command, payload) => {
    const req = (payload as { request: Request }).request;
    if (req.method === "getCatalog") return waiting;
    if (req.method === "keepTabChats") return { method: req.method };
    throw new Error(`Unexpected request: ${req.method}`);
  });
  return answer;
}

test("a catalog read cannot erase a session created on the live feed while it was in flight", async () => {
  useApp.setState({ projects: {}, conversations: {} });
  const answer = deferredCatalog({ projects: [], conversations: [], settings: useApp.getState().settings });
  const loading = loadCatalog();
  applyEvents([event({ type: "projectCreated", project }), event({ type: "conversationCreated", conversation: session })]);
  assert.equal(useApp.getState().conversations.session?.projectId, project.id);
  answer(); await loading;
  assert.equal(useApp.getState().conversations.session?.projectId, project.id);
  assert.equal(useApp.getState().projects.project?.name, project.name);
});

test("an old catalog cannot resurrect removed rows or undo live renames", async () => {
  useApp.setState({ projects: { project }, conversations: { session } });
  const answer = deferredCatalog({ projects: [project], conversations: [session], settings: useApp.getState().settings });
  const loading = loadCatalog();
  applyEvents([event({ type: "conversationRenamed", id: session.id, title: "Live name" })]);
  answer(); await loading;
  assert.equal(useApp.getState().conversations.session?.title, "Live name");
  const answerAgain = deferredCatalog({ projects: [project], conversations: [session], settings: useApp.getState().settings });
  const loadingAgain = loadCatalog();
  applyEvents([event({ type: "conversationDeleted", id: session.id }), event({ type: "projectRemoved", id: project.id })]);
  answerAgain(); await loadingAgain;
  assert.equal(useApp.getState().conversations.session, undefined);
  assert.equal(useApp.getState().projects.project, undefined);
});


test("a fresh catalog still removes old cached rows that had no live change", async () => {
  useApp.setState({ projects: { project }, conversations: { session } });
  const answer = deferredCatalog({ projects: [], conversations: [], settings: useApp.getState().settings });
  const loading = loadCatalog();
  answer(); await loading;
  assert.deepEqual(useApp.getState().projects, {});
  assert.deepEqual(useApp.getState().conversations, {});
});
