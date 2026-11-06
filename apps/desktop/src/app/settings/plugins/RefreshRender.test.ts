import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixturePage } from "@/fixtures/headless";

test("only the Refresh button checks, closing decides nothing, Apply sends each choice, and Add brings a declined item back", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "../src/app/settings/plugins/Refresh.fixture.html", "result", 15000)) as {
    error?: string; askedOnOpen: number; prompt: string; afterNotNow: string[]; missingCard: string; nothing: string; unread: string;
    requests: { method: string; add?: string[]; decline?: string[]; remove?: string[]; keep?: string[]; key?: string }[];
  };
  assert.equal(seen.error, undefined);
  assert.equal(seen.askedOnOpen, 0);
  assert.deepEqual(seen.afterNotNow, ["checkPluginChanges"]);
  for (const name of ["sentry", "pdf-tools", "changelog-writer", "old-deploy", "notion"]) assert.ok(seen.prompt.includes(name), name);
  assert.ok(seen.prompt.includes("Skill in Claude Code + Codex"));
  assert.doesNotMatch(seen.prompt, /\brow\b|catalog|sidecar/i);
  const apply = seen.requests.find((request) => request.method === "applyPluginChanges")!;
  assert.deepEqual(apply.add, ["sentry", "pdf-tools"]);
  assert.deepEqual(apply.decline, ["changelog"]);
  assert.deepEqual(apply.remove, ["server:notion"]);
  assert.deepEqual(apply.keep, ["skill:old-deploy"]);
  assert.match(seen.missingCard, /old-deploy.*Missing/);
  assert.match(seen.nothing, /^Nothing changed/);
  assert.match(seen.unread, /^Couldn't read \/home\/\.claude\.json, so nothing counts as removed/);
  assert.deepEqual(seen.requests.at(-1), { method: "restorePlugin", key: "changelog" });
});
