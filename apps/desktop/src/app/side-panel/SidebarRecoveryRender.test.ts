import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixtureLive } from "@/fixtures/headless";

for (const draft of [false, true]) test(`Source and Review recover after reconnect, draft=${draft}`, { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, `source-freshness.html?audit=1${draft ? "&draft=1" : ""}`, "source-freshness-result", 30000));
  assert.equal(seen.error, undefined, JSON.stringify(seen));
  assert.deepEqual(seen.checks, {
    reviewRefreshesWithPaneClosed: true,
    closedSourceRereadsOnReconnect: true,
    reviewRereadsOnReconnect: true,
    watchSetResentOnReconnect: true,
  });
});


for (const draft of [false, true]) test(`Files recover after reconnect, draft=${draft}`, { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, `source-freshness.html?auditFiles=1${draft ? "&draft=1" : ""}`, "source-freshness-result", 30000));
  assert.equal(seen.error, undefined, JSON.stringify(seen));
  assert.deepEqual(seen.checks, { fileRereadsOnReconnect: true, treeRereadsOnReconnect: true });
});
