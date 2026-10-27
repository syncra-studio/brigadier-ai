import { useCallback, useEffect, useRef, useState } from "react";

import type { CliTarget } from "@/app/conversation/cliCommands";
import { request } from "@/ipc/client";
import type { CliCommand, Request } from "@/ipc/generated";
import { useApp } from "@/state/store";

type Asked = Omit<Extract<Request, { method: "listCliCommands" }>, "method">;
const NONE: readonly CliCommand[] = [];

export function cliCommandsCacheKey(question: Asked, version: string, account: unknown): string {
  return JSON.stringify([question, version, account]);
}

function asked(target: CliTarget | null): Asked | null {
  if (target === null) return null;
  if ("conversation" in target) return { conversationId: target.conversation.id };
  return {
    provider: target.provider,
    ...(target.projectId ? { projectId: target.projectId } : {}),
    ...(target.repo ? { repo: target.repo } : {}),
  };
}

/** A read-only refresh at mount and menu open, versioned by choices and catalog. */
export function useCliCommands(target: CliTarget | null): [readonly CliCommand[], () => void] {
  const question = asked(target);
  const questionKey = question && JSON.stringify(question);
  const settings = useApp((state) => state.settings);
  const accountKey = JSON.stringify([settings.accounts, target && "conversation" in target ? target.conversation.setup : null]);
  const identity = JSON.stringify([questionKey, accountKey]);
  const generation = useRef(0);
  const [answer, setAnswer] = useState<{ identity: string; key: string; commands: readonly CliCommand[] } | null>(null);
  const refresh = useCallback(() => {
    if (questionKey === null) return;
    const current = ++generation.current;
    const question = JSON.parse(questionKey) as Asked;
    // Await the version before the list, so an edit in flight never puts an older list into a
    // newer version's slot. A failed refresh clears the old slot rather than reviving off skills.
    request({ method: "getPluginCatalog" }).then(async ({ catalog }) => {
      const key = cliCommandsCacheKey(question, catalog.version, accountKey);
      const { commands } = await request({ method: "listCliCommands", ...question });
      if (current !== generation.current) return;
      setAnswer({ identity, key, commands });
    }).catch((error: unknown) => {
      if (current !== generation.current) return;
      setAnswer(null);
      console.warn("couldn't list the CLI's commands", error);
    });
  }, [questionKey, accountKey, identity]);
  useEffect(() => {
    refresh();
    return () => { generation.current += 1; };
  }, [refresh]);
  return [answer?.identity === identity ? answer.commands : NONE, refresh];
}
