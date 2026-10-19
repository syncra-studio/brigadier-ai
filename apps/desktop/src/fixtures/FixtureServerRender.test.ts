import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import { fixtureServer, fixtureServersStarted, renderFixtureLive, renderFixturePage } from "@/fixtures/headless";

// #338: a render's time is its page's, not a cold dev server's.
test("renders share one Vite server whose dependencies were optimized before the first", { timeout: 120000 }, async (t) => {
  const server = await fixtureServer();
  await renderFixturePage(t, "badge.html", "badge-result");
  await renderFixtureLive(t, "badge.html", "badge-result", 30000);
  await renderFixturePage(t, "badge.html", "badge-result");
  // The run's server (scripts/run-tests.mjs), or the one this file started.
  assert.equal(fixtureServersStarted(), process.env.BRIGADIER_FIXTURE_SERVER ? 0 : 1);
  assert.equal((await fixtureServer()).pid, server.pid);
  const log = readFileSync(server.log, "utf8");
  assert.ok((log.match(/^> \/fixtures\/badge\.html$/gm) ?? []).length >= 3, "it served each render");
  assert.doesNotMatch(log, /^optimizer: /m, "no dependency was optimized again after a page loaded");
});
