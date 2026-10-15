import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixturePage } from "@/fixtures/headless";

test("approval cards show safe session scopes, dispatch decisions and stack on narrow cards", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "approvals.html?drive=1", "approvals-result")) as {
    buttons: Record<string, string[]>;
    narrow: { x: number; y: number; width: number; height: number }[];
    overflow: string[];
    calls: { id: string; decision: string }[];
    network: string | null;
    networkBody: string | null;
  };
  assert.equal(seen.network, "Allow connecting to https://example.com?");
  assert.equal(seen.networkBody, "example.com isn’t on this session’s list of allowed sites.");
  for (const [id, label] of [
    ["command", "Allow git push for this session"],
    ["network", "Allow example.com for this session"],
    ["files", "Allow workspace edits for this session"],
  ]) assert.deepEqual(seen.buttons[id!], ["DenyEsc", label, "Allow once"]);
  for (const id of ["outside", "permissions", "tool", "unscoped-command"]) {
    assert.deepEqual(seen.buttons[id], ["DenyEsc", "Allow once"]);
  }
  assert.deepEqual(seen.buttons["peer"], ["Don't sendEsc", "Send"]);
  assert.deepEqual(seen.calls, [
    // A window-wide Enter allows nothing: allowing takes a click or Enter on a focused Allow.
    { id: "command", decision: "allowSimilar" },
    { id: "command", decision: "deny" },
    { id: "network", decision: "allowSimilar" },
    { id: "files", decision: "allowSimilar" },
  ]);
  assert.equal(seen.narrow.length, 3);
  const [deny, session, once] = seen.narrow;
  assert.ok(session!.y >= deny!.y + deny!.height);
  assert.ok(once!.y >= session!.y + session!.height);
  assert.equal(session!.x, deny!.x);
  assert.equal(session!.width, deny!.width);
  assert.equal(once!.width, deny!.width);
  assert.deepEqual(seen.overflow, []);
});
