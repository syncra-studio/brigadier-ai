import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixtureLive } from "@/fixtures/headless";

test("the context card reads in the reference order, one row-height line per row, a line per worker while a few work, the branch row (Q91: no Changes row), the Job card under it, and its clicks open things instead of growing it", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "context-rows.html?drive=1", "context-rows-result", 60000)) as Record<string, unknown>;
  assert.deepEqual(seen.sections, ["todo-app", "Workers", "Background", "Sources"]);
  assert.equal(seen.projectMenu, false, "the project row is plain: no ⋯");
  assert.deepEqual(seen.rowHeights, [28], JSON.stringify(seen.odd));
  assert.deepEqual(seen.workers, ["API routes working+48 −0", "Web page working+120 −4", "1 done"]);
  assert.deepEqual(seen.project, ["brigadier/web-ui+380 −2"]);
  // The plan is the Job card's (Q88), under the context card.
  assert.equal(seen.planInCard, 0);
  // The Job card names the plan's step at work, over the plan's phase bar: no fixed Building phase.
  assert.equal(seen.job, "API routes");
  assert.equal(seen.jobAfterCard, true);
  assert.equal(seen.branchTip, "brigadier/web-ui");
  assert.equal(seen.findingsInCard, 0, "no findings list in the card");
  assert.equal(seen.jobOpens, "panel:reviews", "the Job card opens its tab");
  assert.equal(seen.branchIsButton, false, "the branch row is a plain line, as before Q87");
  assert.equal(seen.grew, false);
  // Sources lists what the session used (integrations, then one Web search), then View all,
  // which opens the Sources tab with every search and page (Q87).
  assert.deepEqual(seen.sources, ["Linear", "Web search", "View all"]);
  assert.equal(seen.sourcesOpens, "panel:sources");
  // MCP servers first, under each agent's name (w85-reply-1): the lead's as its CLI said at start.
  assert.deepEqual(seen.mcp, [["Lead · Claude", "linear2 tools", "figmaNeeds sign-in"], ["API routes", "postgresFailed"]]);
  assert.deepEqual(seen.sourcesTab, [
    ["MCP servers"],
    ["Used", "Linear"],
    ["Searched 1 time", "express static files"],
    ["Opened 2 pages", "expressjs.comhttps://expressjs.com/en/starter/static-files.html", "developer.mozilla.orghttps://developer.mozilla.org/en-US/docs/Web/HTML/Element/form"],
  ]);
});
