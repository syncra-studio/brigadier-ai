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
