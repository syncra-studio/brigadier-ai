import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

type State = {
  title: string;
  sections: string[];
  text: string;
  tiles: string[];
  switches: { label: string; checked: string; disabled: boolean }[];
  tile: { padding: number[]; radius: number; gap: number } | null;
  overflow: boolean;
};

test("plugins: drawn server states, switch, details, search and config refresh", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "integrations.html?drive=servers", "integrations-result", 10000)) as Record<string, unknown> & Record<string, State>;
  assert.equal(seen.error, undefined);
  const catalog = seen.servers!;
  assert.equal(catalog.title, "Plugins");
  assert.deepEqual(catalog.sections, ["Yours", "brigadier-ai"]);
  assert.equal(catalog.tiles.length, 7);
  assert.equal(catalog.switches.length, 6, "an unreadable server has no switch");
  assert.equal(catalog.switches.find((item) => item.label === "playwright")?.checked, "false");
  assert.deepEqual(catalog.tile, { padding: [10, 12], radius: 12, gap: 12 });
  assert.match(catalog.text, /Couldn't read it: unknown type “websocket”/);
  assert.match(catalog.text, /Not signed in for Codex/);
  assert.match(catalog.text, /scratch and old-landing aren't trusted/);
  assert.deepEqual(seen.linear!.sections, ["What it is", "Found in", "Workers"]);
  assert.match(seen.linear!.text, /Each agent keeps its own\. Signed in for Claude Code; not signed in for Codex\./);
  assert.match(seen.linear!.text, /codex mcp login linear/);
  assert.match(seen.pencil!.text, /Claude Code, Codex and Claude Desktop/);
  assert.match(seen.pencil!.text, /pencil mcp --stdio/);
  assert.deepEqual(seen.unreadable!.sections, ["Couldn't read it", "What it read", "Found in", "Workers"]);
  assert.equal(seen.unreadable!.switches.length, 0);
  assert.match(seen.unreadable!.text, /~\/.cursor\/mcp.json · line 22/);
  assert.match(seen["empty-servers"]!.text, /No MCP servers found/);
  assert.match(seen["empty-servers"]!.text, /3 trusted projects/);
  assert.equal(seen["empty-servers"]!.tiles.length, 0);
  assert.match(seen["long-servers"]!.text, /northwind-traders-mobile-monorepo-2026-q4-migration/);
  for (const state of ["servers", "search", "linear", "pencil", "unreadable", "empty-servers", "long-servers", "narrow-servers"]) {
    assert.equal(seen[state]!.overflow, false, state);
  }
  assert.equal(seen.switchedOff, true);
  assert.equal(seen.opened, "linear");
  assert.deepEqual(seen.shown, ["~/.claude.json"]);
  assert.deepEqual(seen.filtered, ["Open railway"]);
  assert.match(String(seen.searchResults), /railwayPlugins › MCP serversuse-railwayPlugins › Skills/);
  assert.equal(seen.revealed, true);
  assert.match(seen.reusable!.text, /Codex can use the sign-in from Claude Code/);
  assert.ok(!seen.reusable!.text.includes("mcp login"));
  assert.ok(!seen.reusable!.text.includes("set up in Codex and signed in there"));
  assert.match(seen.unsigned!.text, /Not signed in for Codex, so Codex workers can't use it/);
  assert.match(seen.unsigned!.text, /codex mcp login 'linear team'/);
  assert.ok(!seen.unsigned!.text.includes("signed in for Claude Code only"));
  assert.equal((seen.refreshed as unknown as string[]).length, 6);
  assert.ok(!(seen.refreshed as unknown as string[]).includes("Open figma-dev"));
});
