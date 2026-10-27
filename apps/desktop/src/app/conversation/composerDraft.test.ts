import assert from "node:assert/strict";
import { test } from "node:test";

import { keepWhole, putBack } from "@/app/conversation/composerDraft";
import { loadDraft } from "@/state/drafts";
import type { Mention } from "@/ipc/generated";

test("skill chips survive whole-draft saving and refused-send restoration with their exact row ids", () => {
  const values = new Map<string, string>();
  Object.defineProperty(globalThis, "localStorage", { configurable: true, value: {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => values.set(key, value),
    removeItem: (key: string) => values.delete(key),
  } });
  try {
    const hello: Mention = { type: "skill", rowId: "skill:proj-hello:hash" };
    const agents: Mention = { type: "skill", rowId: "skill:proj-agents:hash" };
    keepWhole("skill-draft", { text: "/proj-hello explain @src/file", attachments: [], mentions: [hello, { type: "file", path: "src/file" }] });
    assert.deepEqual(loadDraft("skill-draft")?.mentions, [
      { at: 0, name: "/proj-hello", mention: hello },
      { at: 20, name: "src/file", mention: { type: "file", path: "src/file" } },
    ]);
    putBack("skill-draft", { text: "/proj-agents other", attachments: [], mentions: [agents] });
    assert.deepEqual(loadDraft("skill-draft")?.mentions?.map(({ at, mention }) => ({ at, mention })), [
      { at: 0, mention: agents }, { at: 19, mention: hello }, { at: 39, mention: { type: "file", path: "src/file" } },
    ]);
    keepWhole("skill-draft", { text: "src/proj-hello /proj-hello-long", attachments: [], mentions: [hello] });
    assert.deepEqual(loadDraft("skill-draft")?.mentions, []);
  } finally {
    Reflect.deleteProperty(globalThis, "localStorage");
  }
});
