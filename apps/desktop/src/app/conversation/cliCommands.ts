import type { ReactNode } from "react";

import type { ComposerCommand } from "@/components/conversation";
import type { CliCommand, Conversation, Mention, ProviderKind } from "@/ipc/generated";

export type CliTarget =
  | { conversation: Conversation }
  | { provider: ProviderKind; projectId: string | null; repo: string | null };

export function conversationProvider(conversation: Conversation): ProviderKind | null {
  const { setup } = conversation;
  return setup?.type === "session" ? setup.orchestrator.provider : setup?.type === "chat" ? setup.model.provider : null;
}

/** A chip's stable textual label can be restored from its persisted row identity. */
export function skillLabel(rowId: string): string {
  return `/${rowId.split(":")[1] ?? rowId}`;
}

function skillTokenStarts(text: string, label: string): number[] {
  const found: number[] = [];
  let at = text.indexOf(label);
  while (at >= 0) {
    if ((at === 0 || /\s/.test(text[at - 1] ?? "")) && !/[\p{L}\p{N}_\-./]/u.test(text[at + label.length] ?? "")) found.push(at);
    at = text.indexOf(label, at + label.length);
  }
  return found;
}

export function hasSkillToken(text: string, label: string): boolean {
  return skillTokenStarts(text, label).length > 0;
}

/** A recalled Codex message may have several chips; Claude keeps its first skill. */
export function limitClaudeSkills(text: string, known: Iterable<Mention>): string {
  const labels = new Set(Array.from(known).flatMap((mention) => mention.type === "skill" ? [skillLabel(mention.rowId)] : []));
  const spans = Array.from(labels).flatMap((label) => skillTokenStarts(text, label).map((at) => ({ at, length: label.length }))).toSorted((a, b) => a.at - b.at);
  for (const { at, length } of spans.slice(1).reverse()) text = text.slice(0, at) + text.slice(at + length);
  return text;
}

export function canPickSkill(provider: ProviderKind | null, text: string, known: Iterable<Mention>): boolean {
  return provider !== "claude" || !Array.from(known).some((mention) => mention.type === "skill" && hasSkillToken(text, skillLabel(mention.rowId)));
}

/** Fixed group order is shared by both CLIs. Skill source is the resolver's row id. */
export function cliMenuCommands(
  listed: readonly CliCommand[],
  own: readonly ComposerCommand[],
  cli: { name: string | undefined; icon: ReactNode; runCommand?: (invocation: string) => void },
): ComposerCommand[] {
  const taken = new Set(own.map((command) => command.id.toLowerCase()));
  const order = { action: 0, skill: 1, command: 2, prompt: 3 };
  return listed
    .filter((command) => command.name.toLowerCase() !== "fusion" && !/^\(removed\)/i.test(command.name) && (command.kind === "skill" || !taken.has(command.name.toLowerCase())))
    .toSorted((a, b) => order[a.kind ?? "command"] - order[b.kind ?? "command"] || a.name.localeCompare(b.name))
    .map((command) => {
      const kind = command.kind ?? "command";
      const name = command.name.split(":").at(-1) ?? command.name;
      const description = command.description ?? command.argumentHint;
      return {
        id: kind === "skill" ? `skill:${command.source}` : `/${command.invocation ?? command.name}`,
        label: name,
        ...(description ? { description } : {}),
        group: kind === "skill" ? "Skills" : kind === "prompt" ? "MCP prompts" : kind === "action" ? "Brigadier actions" : `${cli.name ?? "CLI"} commands`,
        ...(command.agentOnly ? { trailing: command.agentOnly === "claude" ? "Claude Code" : "Codex" } : {}),
        icon: cli.icon,
        ...(kind === "skill" && command.source
          ? { chip: { id: `skill:${command.source}`, type: "skill", label: skillLabel(command.source) } }
          : cli.name === "Codex" && command.kind === "command"
            ? { run: () => cli.runCommand?.(command.invocation ?? "") }
            : { text: command.text }),
      };
    });
}
