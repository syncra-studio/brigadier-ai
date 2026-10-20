import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

test("the MCP server panel: one line per server, its state in words, its tools (or Connected while it names none) behind a click", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "mcp-server-panel.html", "mcp-server-panel-result", 30000));
  assert.deepEqual(seen.rows, ["github3 tools", "linearNeeds sign-in", "postgresFailed", "figmaConnecting", "docs1 tool", "notionConnected"]);
  // One line each, the context card's row height.
  assert.deepEqual(seen.heights, [28]);
  assert.deepEqual(seen.closed, [true, true, true, true, true, true]);
  // Nothing to press but the rows: signing in is said, not offered.
  assert.equal(seen.buttons, 6);
  assert.equal(seen.failedRed, true);
  assert.deepEqual(seen.expanded, ["true", "true", "false", "false", "false", "false"]);
  assert.deepEqual(seen.opened, ["http·Connectedlist_issues · create_pr · get_diff", "Needs sign-in at start"]);
  assert.equal(seen.closedAgain, true);
});
