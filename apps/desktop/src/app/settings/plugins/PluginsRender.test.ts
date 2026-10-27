import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixturePage } from "@/fixtures/headless";

type Card = { name: string; line: string; chip: string | null; on: boolean; locked: boolean };
type Seen = {
  settingsColumn: boolean; title: string; description: string; columns: number; cards: Card[];
  banners: string[]; radios: string[]; projectSwitches: string[]; differentPerAgent: string;
  advanced: { open: boolean; text: string }[]; removeRow: string; overflow: boolean;
  search: { placeholder: string; fits: boolean } | null;
  dialog: { text: string; editable: boolean; coloured: boolean; buttons: string[]; spills: boolean } | null;
};
const findCard = (seen: Seen, name: string) => {
  const found = seen.cards.find((card) => card.name === name);
  assert.ok(found, `Missing ${name}`);
  return found;
};

test("plugins: durable catalog choices, editable detected items, per-agent note and library actions", { timeout: 120000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "plugins.html?drive=1&width=1280", "plugins-result", 40000)) as Record<string, Seen> & { error?: string; calls: string[] };
  assert.equal(seen.error, undefined);
  for (const state of ["L1", "L2", "D1", "D2", "D3", "D4"]) {
    assert.equal(seen[state]!.settingsColumn, true, state);
    assert.equal(seen[state]!.overflow, false, state);
  }
  assert.equal(seen.L1!.title, "Plugins");
  assert.equal(seen.L1!.columns, 2);
  assert.equal(findCard(seen.L1!, "Context7").chip, "Global");
  assert.equal(findCard(seen.L1!, "Linear").chip, "3 projects");
  assert.equal(findCard(seen.L1!, "Linear").line, "Must sign in");
  assert.equal(findCard(seen.L1!, "Context7").line, "Signed in");
  assert.ok(seen.L1!.cards.every((card) => !card.locked));
  assert.ok(seen.L2!.cards.every((card) => !card.locked));
  assert.equal(findCard(seen.flipped!, "Context7").on, false);
  assert.equal(findCard(seen.remounted!, "Context7").on, false, "toggle persists after remount and catalog reload");
  assert.equal(findCard(seen.detectedOff!, "team-db").on, false);
  assert.equal(findCard(seen.nativeSkillsOff!, "frontend-design").on, false, "~/.agents/skills is editable");
  assert.equal(findCard(seen.nativeSkillsOff!, "pr-review").on, false, "~/.claude/skills is editable");
  assert.deepEqual(seen.scoped!.projectSwitches, seen.scopeRemounted!.projectSwitches);
  assert.equal(seen.scopeRemounted!.projectSwitches.filter((row) => row.endsWith(" true")).length, 2);
  assert.ok(seen.scopeRemounted!.projectSwitches[0]!.endsWith(" true"));
  assert.ok(seen.scopeRemounted!.projectSwitches[3]!.endsWith(" true"));
  assert.equal(seen.D3!.differentPerAgent, "Different per agent");
  assert.deepEqual(seen.D3!.banners, [], "L12 is a small note, without a warning");
  assert.match(seen.D3!.removeRow, /Remove pr-review.*files stay/i);
  assert.match(seen.detected!.removeRow, /Remove .*files stay/i, "detected servers can be hidden without deleting their files");
  assert.deepEqual(seen.fallback!.banners, [], "fallback status is inline, without a banner");
  assert.match(seen.D1!.removeRow, /Brigadier's library/);
  assert.ok(seen.D1!.advanced[0]!.open);
  assert.match(seen.D1!.advanced[0]!.text, /Saved in.*Open$/);
  assert.ok(seen.D4!.radios.every((radio) => !radio.endsWith("locked")));
  assert.equal(seen.F1!.dialog!.editable, false, "Open never edits an original CLI skill");
  assert.equal(seen.F2!.dialog!.coloured, true);
  assert.match(seen.F2!.dialog!.text, /"GITHUB_PERSONAL_ACCESS_TOKEN": "••••••••"/);
  assert.match(seen.R1!.dialog!.text, /Removes it from Brigadier's library/);
  assert.match(seen.adding!.dialog!.text, /Adding it to your library/);
  assert.doesNotMatch(seen.added!.dialog!.text, /terminal too|Each agent adds/);
  assert.match(seen.importList!.dialog!.text, /Import from Claude Desktop/);
  assert.match(seen.importList!.dialog!.text, /Import from Cursor/);
  assert.equal(seen.importList!.dialog!.spills, false);
  assert.ok(seen.afterImport!.cards.some((card) => card.name === "filesystem"));
  assert.ok(seen.calls.includes("setPluginChoice lib:context7 false"));
  assert.ok(seen.calls.includes("setPluginChoiceScope server:linear p-brigadier,p-docs"), seen.calls.join("\n"));
  assert.ok(!seen.calls.some((call) => /sharePlugin|switchPlugin|setPluginScope/.test(call)));
});

test("plugins: one column in a narrow window", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "plugins.html?state=L1&width=760", "plugins-result", 10000)) as Seen & { error?: string };
  assert.equal(seen.error, undefined);
  assert.equal(seen.columns, 1);
  assert.equal(seen.overflow, false);
  assert.equal(seen.search?.placeholder, "Search");
  assert.equal(seen.search?.fits, true);
});
