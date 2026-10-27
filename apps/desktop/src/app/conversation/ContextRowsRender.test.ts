import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixtureLive } from "@/fixtures/headless";

test("the context card reads in the reference order, one row-height line per row, a line per worker while a few work, the branch row (Q91: no Changes row), and its clicks open things instead of growing it", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "context-rows.html?drive=1", "context-rows-result", 60000)) as Record<string, unknown>;
  assert.deepEqual(seen.sections, ["todo-app", "Workers", "Background", "Sources"]);
  assert.equal(seen.backgroundClock, true);
  assert.equal(seen.backgroundRows, 0);
  assert.equal(seen.projectMenu, false, "the project row is plain: no ⋯");
  assert.deepEqual(seen.rowHeights, [28], JSON.stringify(seen.odd));
  assert.deepEqual(seen.workers, ["API routes working+48 −0", "Web page working+120 −4", "1 done"]);
  assert.deepEqual(seen.project, ["brigadier/web-ui+380 −2"]);
  assert.equal(seen.branchTip, "brigadier/web-ui");
  assert.equal(seen.findingsInCard, 0, "no findings list in the card");
  assert.equal(seen.branchIsButton, false, "the branch row is a plain line, as before Q87");
  assert.equal(seen.grew, false);
  // Sources lists what the session's CLI used: attachments first, then the pages it fetched,
  // then the MCP servers it called (S14); then View all, which opens the Sources tab with every
  // search and page (Q87).
  assert.deepEqual(seen.sources, ["expressjs.com", "developer.mozilla.org", "Linear", "View all"]);
  assert.equal(seen.sourcesOpens, "panel:sources");
  // MCP servers first, under each agent's name (w85-reply-1): the session's own, then each worker's from its row.
  assert.deepEqual(seen.mcp, [["Lead · Claude", "linear2 tools", "figmaNeeds sign-in"], ["API routes", "postgresFailed"]]);
  assert.deepEqual(seen.sourcesTab, [
    ["MCP servers"],
    ["Used", "Linear"],
    ["Searched 1 time", "express static files"],
    ["Opened 2 pages", "expressjs.comhttps://expressjs.com/en/starter/static-files.html", "developer.mozilla.orghttps://developer.mozilla.org/en-US/docs/Web/HTML/Element/form"],
  ]);
});

test("Waiting on you lists each worker's permission prompt, question and sign-in under its name, and a click opens that worker", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "context-rows.html?drive=1&waiting=1", "context-rows-result", 60000)) as Record<string, unknown>;
  assert.deepEqual(seen.sections, ["todo-app", "Waiting on you", "Workers", "Background", "Sources"]);
  assert.deepEqual(seen.waiting, [
    ["permission", "Web page Allow npm install express"],
    ["question", "API routes Which port should the server use?"],
    ["signIn", "API routes Sign in to figma"],
  ]);
  assert.equal(seen.waitingOpens, "task-3");
  // Its workers that wait read so in the Workers row too.
  assert.deepEqual(seen.workers, ["API routes needs you+48 −0", "Web page needs you+120 −4", "1 done"]);
});

test("with agent fallback off, a usage limit's Resumes at card has Continue now and Don't resume, and Don't resume leaves no card", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "context-rows.html?drive=1&limit=usage", "context-rows-result", 60000)) as Record<string, unknown>;
  assert.equal(seen.switchProviders, false);
  assert.match(String(seen.wait), /^Resumes at .+in 1h 35m/);
  assert.deepEqual(seen.waitButtons, ["Continue now", "Don't resume"]);
  assert.deepEqual(seen.asked, ["continueWaiting", "dontResume"]);
  assert.equal(seen.waitAfter, null);
});

test("credits that ran out say so, with Continue now only and no resume time", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "context-rows.html?drive=1&limit=credits", "context-rows-result", 60000)) as Record<string, unknown>;
  assert.equal(seen.wait, "Out of credits");
  assert.deepEqual(seen.waitButtons, ["Continue now"]);
  assert.deepEqual(seen.asked, ["continueWaiting"]);
});
