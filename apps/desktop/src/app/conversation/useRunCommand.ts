import { useRef } from "react";

import { overnightCommand, type OvernightCommand } from "@/app/conversation/overnightAdapter";
import { useAction } from "@/app/conversation/useAction";
import type { OvernightRun } from "@/ipc/generated";

/**
 * Sends the user's commands to a run, one at a time: each names the revision and generation the
 * user saw, and a retry of the same command (after an error) keeps its id, so the daemon applies
 * it once.
 */
export function useRunCommand(run: OvernightRun): {
  busy: boolean;
  error: string | null;
  send: (kind: string, job: (command: OvernightCommand) => Promise<unknown>, then?: () => void) => void;
} {
  const action = useAction();
  const locked = useRef(false);
  const retryCommands = useRef(new Map<string, OvernightCommand>());
  const send = (kind: string, job: (command: OvernightCommand) => Promise<unknown>, then?: () => void) => {
    if (locked.current) return;
    locked.current = true;
    const key = `${run.id}:${run.revision}:${run.generation}:${kind}`;
    const command = retryCommands.current.get(key) ?? overnightCommand(run);
    retryCommands.current.set(key, command);
    action.run(async () => {
      try {
        await job(command);
        retryCommands.current.delete(key);
        then?.();
      } finally {
        locked.current = false;
      }
    });
  };
  return { busy: action.busy, error: action.error, send };
}
