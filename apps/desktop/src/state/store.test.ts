import assert from "node:assert/strict";
import { test } from "node:test";

import { applyEvents, replaceCatalog, useApp } from "./store";

test("fresh settings start at Approve for me and keep the Full access notice enabled", () => {
  assert.equal(useApp.getState().settings.defaultPermission, "approveForMe");
  assert.equal(useApp.getState().settings.showFullAccessNotice, true);
});

test("loading saved settings and later events preserve the saved permission choice", (t) => {
  const initial = useApp.getState();
  const originalDocument = Object.getOwnPropertyDescriptor(globalThis, "document");
  Object.defineProperty(globalThis, "document", {
    configurable: true,
    value: { documentElement: { dataset: {} } },
  });
  t.after(() => {
    replaceCatalog([], [], initial.settings);
    useApp.setState(initial);
    if (originalDocument) Object.defineProperty(globalThis, "document", originalDocument);
    else Reflect.deleteProperty(globalThis, "document");
  });
  const saved = { ...initial.settings, defaultPermission: "fullAccess" as const, showFullAccessNotice: false };
  replaceCatalog([], [], saved);
  assert.equal(useApp.getState().settings.defaultPermission, "fullAccess");
  assert.equal(useApp.getState().settings.showFullAccessNotice, false);
  applyEvents([{
    seq: 1,
    stream: "settings",
    streamSeq: 1,
    atMs: 1,
    event: { type: "settingsChanged", settings: { ...saved, defaultPermission: "askForApproval" } },
  }]);
  assert.equal(useApp.getState().settings.defaultPermission, "askForApproval");
});

test("workspace events keep the store's workspaces, and removing a project drops its own", () => {
  useApp.setState({ workspaces: {} });
  const workspace = {
    id: "ws-1", projectId: "p-1", title: "", repos: [], base: "main", createdAtMs: 0, updatedAtMs: 0,
    pinnedAtMs: null, lifecycle: "active" as const,
  };
  let seq = 0;
  const at = (event: Parameters<typeof applyEvents>[0][number]["event"]) => ({ seq: ++seq, stream: "catalog", streamSeq: seq, atMs: seq, event });
  applyEvents([
    at({ type: "workspaceCreated", workspace }),
    at({ type: "workspaceCreated", workspace: { ...workspace, id: "ws-2", projectId: "p-2" } }),
    at({ type: "workspaceRenamed", id: "ws-1", title: "Redesigning the header" }),
    at({ type: "workspacePinned", id: "ws-1", pinnedAtMs: 5 }),
    at({ type: "workspaceArchived", id: "ws-1", archived: true }),
  ]);
  const one = useApp.getState().workspaces["ws-1"];
  assert.deepEqual([one?.title, one?.pinnedAtMs, one?.lifecycle], ["Redesigning the header", 5, "archived"]);
  useApp.setState({ projects: { "p-2": {} as never } });
  applyEvents([at({ type: "projectRemoved", id: "p-2" })]);
  assert.deepEqual(Object.keys(useApp.getState().workspaces), ["ws-1"]);
});
