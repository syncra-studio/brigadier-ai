import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

type State = {
  title: string;
  sections: string[];
  text: string;
  tiles: string[];
  switches: { label: string; checked: string; disabled: boolean }[];
  overflow: boolean;
};

test("skills: drawn states, separate project switches, folders and Claude always on", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "integrations.html?drive=skills", "integrations-result", 10000)) as Record<string, unknown> & Record<string, State>;
  assert.equal(seen.error, undefined);
  const catalog = seen.skills!;
  assert.equal(catalog.title, "Plugins");
  assert.equal(catalog.tiles.length, 20);
  assert.equal(catalog.switches.length, 20);
  assert.ok(catalog.tiles.includes("Open spreadsheets:chart"));
  assert.deepEqual(catalog.sections, ["Yours", "brigadier-ai"]);
  assert.ok(catalog.tiles.includes("Open delegator"));
  assert.ok(catalog.tiles.includes("Open delegator in ~/Code/brigadier-ai"));
  assert.equal(catalog.switches.find((item) => item.label === "grilling")?.checked, "false");
  assert.deepEqual(seen["grill-me"]!.sections, ["Folders", "Workers"]);
  assert.match(seen["grill-me"]!.text, /One skill, in 2 folders/);
  assert.match(seen["grill-me"]!.text, /Linked to ~\/.agents\/skills\/grill-me · Claude reads this folder/);
  assert.match(seen.delegator!.text, /Another skill is called delegator too, in brigadier-ai/);
  assert.match(seen.delegator!.text, /Given by Brigadier/);
  assert.match(seen["claude-skills"]!.text, /Off for Codex workers · Claude workers still have it/);
  assert.match(seen["claude-skill"]!.text, /Claude: always on/);
  assert.match(seen["claude-skill"]!.text, /Off: Brigadier doesn't give it to Codex workers/);
  assert.deepEqual(seen["claude-skill"]!.switches, [{ label: "higgsfield-websites: Codex workers", checked: "false", disabled: false }]);
  assert.equal(seen["plugin-skill"]!.title, "spreadsheets:chart");
  assert.match(seen["plugin-skill"]!.text, /From the spreadsheets@openai-primary-runtime plugin · Codex reads this folder/);
  assert.match(seen["plugin-skill"]!.text, /Claude doesn't read ~\/.codex\/plugins\/cache\/openai-primary-runtime\/spreadsheets\/26.1017.22135\/skills, so Brigadier gives it to them/);
  assert.match(seen["empty-skills"]!.text, /No skills found/);
  assert.equal(seen["empty-skills"]!.tiles.length, 0);
  for (const state of ["skills", "grill-me", "delegator", "claude-skills", "claude-skill", "plugin-skill", "empty-skills", "narrow-skills"]) {
    assert.equal(seen[state]!.overflow, false, state);
  }
  assert.equal(seen.duplicateSwitch, true);
  assert.equal(seen.opened, "grill-me");
  assert.deepEqual(seen.shown, ["~/.agents/skills/grill-me"]);
  assert.deepEqual(seen.filtered, ["Open use-railway"]);
  assert.match(seen.claudeOn!.text, /Claude: always on/);
  assert.match(seen.claudeOn!.text, /Given by Brigadier/);
  assert.equal((seen.refreshed as unknown as string[]).length, 19);
  assert.ok(!(seen.refreshed as unknown as string[]).includes("Open dataviz"));
});
