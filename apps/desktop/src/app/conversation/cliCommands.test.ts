import assert from "node:assert/strict";
import { test } from "node:test";

import { cliMenuCommands } from "@/app/conversation/cliCommands";
import type { ComposerCommand } from "@/components/conversation";
import type { CliCommand } from "@/ipc/generated";

const own: ComposerCommand[] = [
  { id: "model", label: "Model", icon: null, run: () => {} },
  { id: "compact", label: "Compact", icon: null, run: () => {} },
];
const command = (name: string, text: string, description: string | null = null, argumentHint: string | null = null): CliCommand => ({
  name,
  text,
  description,
  argumentHint,
});

test("the / menu lists what Claude Code says it runs, under its name, typed as /name", () => {
  // As `initialize` reports them: built-ins, the user's and the project's skills, MCP prompts.
  const listed = [
    command("grill-me", "/grill-me", "A relentless interview. (user)"),
    command("compact", "/compact", "Clear history but keep a summary"),
    command("w03-proj-skill", "/w03-proj-skill", "Project skill. (project)"),
    command("w03p:greet (MCP)", "/w03p:greet (MCP)", "Greet someone"),
    command("loop", "/loop", null, "[interval] [prompt]"),
  ];
  const rows = cliMenuCommands(listed, own, { name: "Claude Code", icon: "icon" });
  // Brigadier's own Compact stays the only one; the rest by name.
  assert.deepEqual(rows.map((row) => [row.id, row.label, row.text, row.group]), [
    ["/grill-me", "grill-me", "/grill-me", "Claude Code"],
    ["/loop", "loop", "/loop", "Claude Code"],
    ["/w03-proj-skill", "w03-proj-skill", "/w03-proj-skill", "Claude Code"],
    ["/w03p:greet (MCP)", "w03p:greet (MCP)", "/w03p:greet (MCP)", "Claude Code"],
  ]);
  assert.equal(rows[0]?.description, "A relentless interview. (user)");
  // No description: what it takes instead.
  assert.equal(rows[1]?.description, "[interval] [prompt]");
  assert.equal(rows[0]?.run, undefined);
  assert.equal(rows[0]?.icon, "icon");
});

test("the / menu lists Codex's skills under its name, typed as its $name mention", () => {
  const listed = [command("w03-codex-skill", "$w03-codex-skill", "Project skill"), command("pdf:pdf", "$pdf:pdf", "PDFs")];
  const rows = cliMenuCommands(listed, own, { name: "Codex", icon: null });
  assert.deepEqual(rows.map((row) => [row.label, row.text, row.group]), [
    ["pdf:pdf", "$pdf:pdf", "Codex"],
    ["w03-codex-skill", "$w03-codex-skill", "Codex"],
  ]);
});

test("a CLI that reports nothing adds nothing, and an unknown CLI's rows go without a heading", () => {
  assert.deepEqual(cliMenuCommands([], own, { name: "Claude Code", icon: null }), []);
  assert.equal(cliMenuCommands([command("x", "/x")], own, { name: undefined, icon: null })[0]?.group, undefined);
});
