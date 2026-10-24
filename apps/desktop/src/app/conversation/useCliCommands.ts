import { useCallback, useState } from "react";

import type { CliTarget } from "@/app/conversation/cliCommands";
import { request } from "@/ipc/client";
import type { CliCommand, Request } from "@/ipc/generated";

type Asked = Omit<Extract<Request, { method: "listCliCommands" }>, "method">;

/** What each CLI last said it runs, by what it was asked, so a menu opened again shows it at once. */
const known = new Map<string, readonly CliCommand[]>();
const NONE: readonly CliCommand[] = [];

function asked(target: CliTarget | null): Asked | null {
  if (target === null) return null;
  if ("conversation" in target) return { conversationId: target.conversation.id };
  return {
    provider: target.provider,
    ...(target.projectId ? { projectId: target.projectId } : {}),
    ...(target.repo ? { repo: target.repo } : {}),
  };
}

/**
 * What the target's CLI runs from a message, as it last said, and a refresh that asks it again
 * (as the `/` menu opens: a CLI that started since knows its MCP prompts too).
 */
export function useCliCommands(target: CliTarget | null): [readonly CliCommand[], () => void] {
  const question = asked(target);
  const key = question && JSON.stringify(question);
  const [, heard] = useState(0);
  const refresh = useCallback(() => {
    if (key === null) return;
    request({ method: "listCliCommands", ...(JSON.parse(key) as Asked) }).then(
      ({ commands }) => {
        known.set(key, commands);
        heard((count) => count + 1);
      },
      // The menu keeps Brigadier's own commands and what the CLI said before.
      (error: unknown) => console.warn("couldn't list the CLI's commands", error),
    );
  }, [key]);
  return [(key !== null && known.get(key)) || NONE, refresh];
}
