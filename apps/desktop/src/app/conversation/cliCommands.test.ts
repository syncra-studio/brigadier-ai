import assert from "node:assert/strict";
import { test } from "node:test";

import { canPickSkill, cliMenuCommands, hasSkillToken, limitClaudeSkills, skillLabel } from "@/app/conversation/cliCommands";
import { commandMatches } from "@/components/conversation/elements/menuMatch";
import { cliCommandsCacheKey } from "@/app/conversation/useCliCommands";
import type { ComposerCommand } from "@/components/conversation";
import type { CliCommand, Mention } from "@/ipc/generated";

const own: ComposerCommand[] = [
  { id: "model", label: "Model", icon: null, run: () => {} },
  { id: "compact", label: "Compact", icon: null, run: () => {} },
];
const command = (name: string, kind: CliCommand["kind"] = "command", source?: string): CliCommand => ({
  name, kind, ...(source ? { source } : {}), text: `/${name}`, description: null, argumentHint: null,
});

test("both CLIs have one ordered group per kind, plain skill names and stable chips", () => {
  for (const cli of ["Claude Code", "Codex"]) {
    const listed = [
      command("higgsfield-generate:higgsfield-generate", "skill", "skill:higgsfield-generate:abc"),
      command("compact"), command("context"), command("proj-hello", "skill", "skill:proj-hello:hash"),
      ...(cli === "Claude Code" ? [{ ...command("greet", "prompt", "db"), agentOnly: "claude" as const }] : []),
    ];
    const rows = cliMenuCommands(listed, own, { name: cli, icon: "icon" });
    assert.deepEqual(rows.map((row) => row.group), ["Skills", "Skills", `${cli} commands`, ...(cli === "Claude Code" ? ["MCP prompts"] : [])]);
    assert.equal(rows[0]?.label, "higgsfield-generate");
    assert.equal(rows[0]?.text, undefined);
    assert.deepEqual(rows[1]?.chip, { id: "skill:skill:proj-hello:hash", type: "skill", label: "/proj-hello" });
    assert.equal(rows.find((row) => row.label === "greet")?.trailing, cli === "Claude Code" ? "Claude Code" : undefined);
  }
});

test("removed agents and Fusion are omitted", () => {
  assert.deepEqual(cliMenuCommands([command("fusion"), command("(removed) agents")], own, { name: "Codex", icon: null }), []);
});

test("Claude allows at most one skill; removed tokens and Codex permit a new pick", () => {
  const known: Mention[] = [{ type: "skill", rowId: "skill:proj-hello:hash" }];
  assert.equal(skillLabel("skill:proj-hello:hash"), "/proj-hello");
  assert.equal(canPickSkill("claude", "/proj-hello explain", known), false);
  assert.equal(canPickSkill("claude", "explain", known), true);
  assert.equal(canPickSkill("claude", "/proj-hello-world", known), true);
  assert.equal(canPickSkill("codex", "/proj-hello explain", known), true);
  assert.equal(hasSkillToken("src/proj-hello", "/proj-hello"), false);
});

test("choices/catalog and account changes invalidate the module cache key", () => {
  const question = { provider: "codex" as const, projectId: "p" };
  assert.notEqual(cliCommandsCacheKey(question, "before-choice", "own"), cliCommandsCacheKey(question, "after-choice", "own"));
  assert.notEqual(cliCommandsCacheKey(question, "same", "own"), cliCommandsCacheKey(question, "same", "second"));
});


test("Codex compact invokes its mapped RPC action without inserting model text", () => {
  const invoked: string[] = [];
  const rows = cliMenuCommands([{ ...command("compact"), invocation: "thread/compact/start" }], [], { name: "Codex", icon: null, runCommand: (name) => invoked.push(name) });
  assert.equal(rows[0]?.text, undefined);
  rows[0]?.run?.();
  assert.deepEqual(invoked, ["thread/compact/start"]);
});


test("restored messages and a switch to Claude retain only their first skill chip", () => {
  const known: Mention[] = [{ type: "skill", rowId: "skill:proj-agents:one" }, { type: "skill", rowId: "skill:proj-hello:two" }];
  assert.equal(limitClaudeSkills("/proj-hello /proj-agents explain", known), "/proj-hello  explain");
  assert.equal(limitClaudeSkills("/proj-hello /proj-hello explain", known), "/proj-hello  explain");
  assert.equal(limitClaudeSkills("src/proj-hello /proj-agents explain", known), "src/proj-hello /proj-agents explain");
});


test("fuzzy filtering keeps each header together for Claude and Codex", () => {
  for (const cli of ["Claude Code", "Codex"]) {
    const rows = [
      { id: "archive", label: "Archive", group: "Brigadier actions" },
      ...cliMenuCommands([
        command("standards", "skill", "skill:standards:a"), command("agents", "skill", "skill:agents:b"),
        command("alpha"), command("status"),
      ], [], { name: cli, icon: null }),
    ];
    const matched = commandMatches(rows.map((row) => ({ id: row.id, label: row.label, type: "command", metadata: { group: row.group ?? null } })), "a");
    const groups = matched.map((item) => item.metadata?.group).filter((group, index, all) => index === 0 || group !== all[index - 1]);
    assert.deepEqual(groups, ["Brigadier actions", "Skills", `${cli} commands`]);
  }
});
