import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixtureLive } from "@/fixtures/headless";

// #469: the composer holds still as the app starts. Its rail, there under the startup screen,
// doesn't open as the app fades in, and the editor takes the plain field's place at its height.
test("the composer keeps its place as the app starts: no rail opening under the startup screen, the editor as tall as the field it replaces", { timeout: 120000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "composer-startup.html", "composer-startup-result", 90000)) as Record<string, { height: number; field: string; railOpens: boolean | null }>;
  assert.equal((seen as { error?: string }).error, undefined, String((seen as { error?: string }).error));
  assert.equal(seen.underStartup!.field, "stand-in");
  assert.equal(seen.underStartup!.railOpens, false);
  assert.equal(seen.revealed!.field, "editor");
  assert.equal(seen.revealed!.height, seen.underStartup!.height);
  // #564: a new chat has no "Latest response" heading.
  assert.equal((seen as unknown as { latestResponse: number }).latestResponse, 0);
  // A conversation switched to has its rail as it was: nothing closes and opens again.
  assert.equal(seen.switched!.railOpens, false);
  // Once the app shows, a rail item that comes later opens as it appears.
  assert.equal(seen.later!.railOpens, true);
});
