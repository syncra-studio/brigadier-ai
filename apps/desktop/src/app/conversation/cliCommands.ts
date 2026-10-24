import type { ReactNode } from "react";

import type { ComposerCommand } from "@/components/conversation";
import type { CliCommand, Conversation, ProviderKind } from "@/ipc/generated";

/** Whose CLI the `/` menu asks: a conversation's, or a new one's (its CLI, project and folder). */
export type CliTarget =
  | { conversation: Conversation }
  | { provider: ProviderKind; projectId: string | null; repo: string | null };

/** The CLI a conversation runs on: its session's orchestrator's, or its Chat's model's. */
export function conversationProvider(conversation: Conversation): ProviderKind | null {
  const { setup } = conversation;
  return setup?.type === "session" ? setup.orchestrator.provider : setup?.type === "chat" ? setup.model.provider : null;
}

/**
 * The `/` menu's rows for what the CLI says it runs, by name. Picking one types it
 * (`command.text`) in the message. One the menu already has as Brigadier's own (Model, Compact,
 * Init) stays Brigadier's. `cli`: the CLI's name (the rows' heading) and mark.
 */
export function cliMenuCommands(
  listed: readonly CliCommand[],
  own: readonly ComposerCommand[],
  cli: { name: string | undefined; icon: ReactNode },
): ComposerCommand[] {
  const taken = new Set(own.map((command) => command.id.toLowerCase()));
  return listed
    .filter((command) => !taken.has(command.name.toLowerCase()))
    .toSorted((a, b) => a.name.localeCompare(b.name))
    .map((command) => {
      // No description: what it takes instead.
      const description = command.description ?? command.argumentHint;
      return {
        id: `/${command.name}`,
        label: command.name,
        ...(description ? { description } : {}),
        ...(cli.name ? { group: cli.name } : {}),
        icon: cli.icon,
        text: command.text,
      };
    });
}
