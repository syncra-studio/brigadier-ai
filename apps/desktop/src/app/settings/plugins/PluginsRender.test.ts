import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

type Card = { name: string; line: string; tone: string | null; chip: string | null; mark: string | null; on: boolean; locked: boolean; dim: boolean };
type Seen = {
  error?: string;
  settingsColumn: boolean;
  title: string;
  description: string;
  tabs: string[];
  toolbar: string[];
  search: { placeholder: string; label: string; fits: boolean } | null;
  columns: number;
  cards: Card[];
  banners: string[];
  radios: string[];
  projectSwitches: string[];
  sectionLine: string;
  advanced: { open: boolean; text: string }[];
  removeRow: string;
  empty: string;
  dialog: { slot: string; width: number; viewport: number; rem: number; height: number; title: string; text: string; buttons: string[]; editable: boolean; lines: number; coloured: boolean; status: string; banner: string; body: string } | null;
  toasts: string[];
  alert: string;
  overflow: boolean;
};

const names = (seen: Seen) => seen.cards.map((card) => card.name);
const card = (seen: Seen, name: string) => {
  const found = seen.cards.find((entry) => entry.name === name);
  assert.ok(found, `no card ${name}: ${names(seen).join(", ")}`);
  return found;
};

// The page in Chromium over a faked daemon: the list, the details, Open, and what each asks for.
test("plugins: the grid, details and Open, and the requests they make", { timeout: 120000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "plugins.html?drive=1&width=1280", "plugins-result", 40000)) as Record<string, Seen> & { error?: string; calls: string[] };
  assert.equal(seen.error, undefined);
  for (const state of ["L1", "L2", "D1", "D2", "D3", "D4"]) {
    assert.equal(seen[state]!.settingsColumn, true, `${state}: the column every Settings page uses`);
    assert.equal(seen[state]!.overflow, false, `${state}: nothing runs off the page`);
  }

  // L1: the MCP servers tab. No project picker: the two tabs, search and Add.
  const l1 = seen.L1!;
  assert.equal(l1.title, "Plugins");
  assert.equal(l1.description, "Skills and MCP servers your agents can use.");
  assert.deepEqual(l1.tabs, ["MCP servers8 *", "Skills8"]);
  assert.deepEqual(l1.toolbar, ["Add"]);
  assert.deepEqual(l1.search && { placeholder: l1.search.placeholder, label: l1.search.label }, { placeholder: "Search", label: "Search MCP servers" });
  // Two columns in a wide window; the problem first, then A–Z.
  assert.equal(l1.columns, 2);
  assert.deepEqual(names(l1), ["Linear", "Context7", "error-feed", "Figma", "GitHub", "Playwright", "postgres", "team-db"]);
  assert.deepEqual(card(l1, "Linear"), { name: "Linear", line: "Needs sign-in", tone: "warning", chip: "3 projects", mark: "linear", on: true, locked: false, dim: false });
  // Its site's icon, as the daemon sends it; one that doesn't load falls back to the illustration.
  assert.deepEqual(card(l1, "Context7"), { name: "Context7", line: "Up-to-date docs and code examples for the libraries you use.", tone: "muted", chip: "Global", mark: "icon", on: true, locked: false, dim: false });
  assert.equal(card(l1, "error-feed").mark, "server");
  assert.equal(card(l1, "GitHub").mark, "github");
  // No icon of its own: the server illustration.
  assert.equal(card(l1, "Playwright").mark, "server");
  assert.equal(card(l1, "Playwright").chip, "brigadier-ai");
  // Off: dimmed, its tag kept; no description: its command.
  assert.deepEqual(card(l1, "postgres"), { name: "postgres", line: "uvx postgres-mcp postgresql://localhost/steady", tone: "muted", chip: "steady-app", mark: "postgresql", on: false, locked: false, dim: true });
  // Locked: its switch off-limits, the reason as its line, in the usual grey.
  assert.deepEqual(card(l1, "team-db"), { name: "team-db", line: "Shared with your team in steady-app/.mcp.json: change it there.", tone: "muted", chip: "steady-app", mark: "server", on: true, locked: true, dim: false });

  // L2: Skills. The items only one agent has come first, their line in warning colour.
  const l2 = seen.L2!;
  assert.deepEqual(l2.tabs, ["MCP servers8", "Skills8 *"]);
  assert.deepEqual(names(l2), ["frontend-design", "pr-review", "commit-messages", "pdf-forms", "release-notes", "rust-style", "tailwind-tokens", "webapp-testing"]);
  assert.deepEqual(card(l2, "frontend-design"), { name: "frontend-design", line: "Only in Claude Code: came in Claude Code's own plugin.", tone: "warning", chip: "Global", mark: "skill", on: true, locked: false, dim: false });
  assert.equal(card(l2, "pr-review").line, "Only in Claude Code");
  assert.equal(card(l2, "pdf-forms").chip, "2 projects");
  assert.equal(card(l2, "tailwind-tokens").line, "/Users/me/.brigadier/library/plugins/tailwind-tokens/skills/tailwind-tokens");

  // L3: nothing yet.
  assert.match(seen.L3!.empty, /No MCP servers yet/);

  // D1: a server's details. Its banner, only some projects with a switch each, Advanced open, Remove.
  const d1 = seen.D1!;
  assert.equal(d1.title, "Linear");
  assert.deepEqual(d1.banners, ["Linear needs you to sign in. Until then, sessions can't use it.Sign in"]);
  assert.deepEqual(d1.radios, ["All projectsOn in every project, and in new ones you add.", "Only some projectsOn only in the projects you pick. *"]);
  assert.deepEqual(d1.projectSwitches, [
    "brigadier-ai~/Code/brigadier-ai true",
    "steady-app~/Code/steady-app true",
    "landingCodex only reads this project's settings once you trust the project.Trust landing true",
    "docs-site~/Code/docs-site false",
  ]);
  assert.equal(d1.advanced.length, 1);
  assert.equal(d1.advanced[0]!.open, true);
  assert.match(d1.advanced[0]!.text, /^AdvancedAddress, where it's saved, and its settings\.Addresshttps:\/\/mcp\.linear\.app\/mcp.*Saved in.*SettingsIts address and header names\. Secret values stay hidden\.Open$/);
  assert.doesNotMatch(d1.advanced[0]!.text, /agent/i);
  assert.equal(d1.removeRow, "Remove LinearTakes out what Brigadier added, from every agent and project.");

  // D2: a skill's details: every project, Folder and SKILL.md in Advanced.
  const d2 = seen.D2!;
  assert.deepEqual(d2.banners, []);
  assert.deepEqual(d2.radios, ["All projectsOn in every project, and in new ones you add. *", "Only some projectsOn only in the projects you pick."]);
  assert.deepEqual(d2.projectSwitches, []);
  assert.match(d2.advanced[0]!.text, /Folder.*SKILL\.mdRead or edit the skill's instructions\.Open$/);

  // D3: an item removed earlier, only in Claude Code: Share with every agent, and no Remove.
  const d3 = seen.D3!;
  assert.deepEqual(d3.banners, ["Only in Claude CodeShare with every agent"]);
  assert.equal(d3.removeRow, "");

  // D4: a locked item: the choice and the switches off-limits, the reason under Projects.
  const d4 = seen.D4!;
  assert.equal(d4.sectionLine, "Shared with your team in steady-app/.mcp.json: change it there.");
  assert.ok(d4.radios.every((radio) => radio.endsWith("locked")), d4.radios.join(" | "));
  assert.ok(d4.projectSwitches.every((row) => row.endsWith("locked")), d4.projectSwitches.join(" | "));
  assert.equal(d4.removeRow, "");

  // F1: a skill's SKILL.md, editable, wide, numbered like the file tab.
  const f1 = seen.F1!.dialog!;
  assert.equal(f1.slot, "plugin-file");
  // Wide: 96rem, or the window less a margin.
  assert.equal(f1.width, Math.round(Math.min(96 * f1.rem, f1.viewport - 4 * f1.rem)));
  assert.equal(f1.editable, true);
  assert.equal(f1.lines, 11);
  assert.match(f1.title, /^SKILL\.md ~\/\.brigadier\/library\/plugins\/release-notes\/skills\/release-notes$/);
  assert.deepEqual(f1.buttons.filter((button) => ["Copy path", "Reveal in Finder", "Close", "Cancel", "Save"].includes(button)), ["Copy path", "Reveal in Finder", "Close", "Cancel", "Save"]);

  // F2: a server's settings: read only, coloured, its secret's name without its value.
  const f2 = seen.F2!.dialog!;
  assert.equal(f2.editable, false);
  assert.equal(f2.title, "GitHub.json");
  assert.match(f2.text, /Read only/);
  assert.match(f2.text, /"GITHUB_PERSONAL_ACCESS_TOKEN": "••••••••"/);
  assert.equal(f2.coloured, true);
  assert.ok(!f2.buttons.includes("Save"));

  // R1: Remove of an item an agent has of its own says its original stays.
  assert.match(seen.R1!.dialog!.text, /Brigadier takes out only the copies it gave your other agents\. Your own original stays as it is/);

  // The switch, the scope, Share, Remove, Save and Add each ask the daemon for one thing.
  assert.equal(card(seen.flipped!, "Context7").on, false);
  assert.deepEqual(seen.scoped!.projectSwitches.at(-1), "docs-site~/Code/docs-site true");
  assert.equal(seen.everywhere!.radios[0], "All projectsOn in every project, and in new ones you add. *");
  assert.equal(seen.someAgain!.projectSwitches.filter((row) => row.endsWith(" true")).length, 4);
  assert.equal(seen.edited!.dialog!.status, "Unsaved changes");
  assert.equal(seen.escaped!.dialog?.slot, "plugin-file", "Escape doesn't drop unsaved edits");
  assert.equal(seen.savedClosed!.dialog, null);
  assert.equal(seen.reopened!.dialog!.lines, 12);
  // A save over a file changed since it opened is refused, plainly; Reload shows it as it is now.
  assert.equal(seen.refused!.dialog?.slot, "plugin-file");
  assert.match(seen.refused!.dialog!.banner, /^SKILL\.md changed on disk after you opened it, so Save didn't write over it\. .*Reload$/);
  assert.equal(seen.reloaded!.dialog!.banner, "");
  assert.match(seen.reloaded!.dialog!.body, /4\. Theirs\./);
  assert.doesNotMatch(seen.reloaded!.dialog!.body, /4\. Mine\./);
  assert.deepEqual(seen.shared!.banners, []);
  // Its original stays, so its details do too: now only in Claude Code, to share again.
  assert.equal(seen.removedCopies!.title, "GitHub");
  assert.deepEqual(seen.removedCopies!.banners, ["Only in Claude CodeShare with every agent"]);
  assert.equal(seen.removedCopies!.removeRow, "");
  assert.doesNotMatch(seen.addReview!.dialog!.text, /agents you picked|Share with the team/);
  assert.match(seen.adding!.dialog!.text, /Adding it to every agent…/);
  assert.match(seen.added!.dialog!.text, /Claude CodeAddedDone.*CodexAddedDone/);
  assert.deepEqual(names(seen.searched!), ["GitHub"]);
  assert.equal(seen.noMatch!.cards.length, 0);
  assert.deepEqual(seen.calls.filter((call) => !call.startsWith("getPluginItem") && !call.startsWith("checkPluginAdd")), [
    "switchPlugin lib:context7 false",
    "setPluginScope server:linear p-brigadier,p-steady,p-landing,p-docs",
    "setPluginScope server:linear all",
    "setPluginScope server:linear p-brigadier,p-steady,p-landing,p-docs",
    "savePluginSkill lib:release-notes /Users/me/.brigadier/library/plugins/release-notes/skills/release-notes/SKILL.md",
    "savePluginSkill lib:release-notes /Users/me/.brigadier/library/plugins/release-notes/skills/release-notes/SKILL.md",
    "sharePlugin skill:pr-review",
    "removePlugin server:github",
    "addPlugin - all",
  ]);
});

// A narrow window: one column, and the whole of "Search" shows.
test("plugins: one column in a narrow window", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "plugins.html?state=L1&width=760", "plugins-result", 10000)) as Seen;
  assert.equal(seen.error, undefined);
  assert.equal(seen.columns, 1);
  assert.equal(seen.overflow, false);
  assert.equal(seen.search?.placeholder, "Search");
  assert.equal(seen.search?.fits, true, "the field is wide enough for its word");
  assert.equal(seen.cards[0]!.name, "Linear");
});
