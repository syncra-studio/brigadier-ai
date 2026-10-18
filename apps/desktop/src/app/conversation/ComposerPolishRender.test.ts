import assert from "node:assert/strict";
import { test } from "node:test";
import type { TestContext } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

async function polish(t: TestContext, which: string): Promise<Record<string, unknown>> {
  const seen = JSON.parse(await renderFixtureLive(t, `composer-polish.html?case=${which}`, "composer-polish-result", 60000)) as Record<string, unknown>;
  assert.equal(seen.error, undefined, String(seen.error));
  return seen;
}

test("Find in the chat keeps the keyboard: the composer focuses itself (on load, as a run starts) only when no other field has it (issue 304)", { timeout: 90000 }, async (t) => {
  const seen = await polish(t, "focus");
  assert.equal(seen.runStart, "Find in chat");
  assert.equal(seen.runStartFree, "Message input");
});

test("Send waits while attached files upload: disabled, its tip Files uploading…, and Enter sends nothing until they are stored (issue 302)", { timeout: 90000 }, async (t) => {
  const seen = await polish(t, "uploading");
  assert.deepEqual(seen.whileUploading, { disabled: true, tip: "Files uploading…" });
  assert.equal(seen.enterWhileUploading, false);
  assert.deepEqual(seen.afterUpload, { disabled: false, tip: "Send message" });
});
