import { useEffect, type ReactNode } from "react";

import { PROVIDER_LABELS } from "@/app/inspector/providers/shared";
import { useAction } from "@/app/conversation/useAction";
import { PROJECT_DIALOG_CARD, PROJECT_DIALOG_TITLE } from "@/app/shell/dialogs/fields";
import { Button, ErrorLine } from "@/components/controls";
import { X } from "@/components/icons";
import { Dialog, DialogClose, DialogContent, DialogDescription, DialogTitle } from "@/components/overlays";
import { cn } from "@/lib/utils";
import type { PluginAgentResult, PluginsAdapter } from "./adapter";
import { type PluginAgent } from "./parts";
import type { PluginItemView } from "./types";

/** How often a dialog that waits on the agents reads the page again. */
const WATCH_MS = 1000;

/** The Plugins dialogs' shell: the app's dialog at Project settings' width, a title, a line, a close. */
export function PluginDialog({ open = true, onClose, title, description, slot, children }: {
  open?: boolean;
  onClose: () => void;
  title: string;
  description: ReactNode;
  /** Its name in tests: "add", "sign-in"… */
  slot: string;
  children: ReactNode;
}) {
  return (
    <Dialog open={open} onOpenChange={(value) => !value && onClose()}>
      <DialogContent data-dialog={slot} showCloseButton={false} className={cn(PROJECT_DIALOG_CARD, "max-w-project-dialog max-h-full gap-6 overflow-y-auto p-6")}>
        <div className="flex flex-col gap-3">
          <div className="flex items-center gap-3">
            <DialogTitle className={cn(PROJECT_DIALOG_TITLE, "min-w-0 flex-1")}>{title}</DialogTitle>
            <DialogClose asChild>
              <Button variant="ghost" size="icon-sm" className="text-muted-foreground" aria-label="Close">
                <X />
              </Button>
            </DialogClose>
          </div>
          <DialogDescription className="text-foreground/65 text-sm">{description}</DialogDescription>
        </div>
        {children}
      </DialogContent>
    </Dialog>
  );
}

/** An agent as the dialogs name it: "Claude Code", or its id for one the app has no name for. */
export const agentOf = (agent: string): PluginAgent => ({ agent, label: (PROVIDER_LABELS as Record<string, string>)[agent] ?? agent });

/** The agents that didn't take a change, with why: "Codex: …". */
export function failures(results: PluginAgentResult[]): string | null {
  const failed = results.filter((result) => !result.ok);
  if (!failed.length) return null;
  return failed.map((result) => `${agentOf(result.agent).label}: ${result.message ?? "it didn't take the change."}`).join("\n");
}

/** What the user should know although every agent took a change: "sentry is added, but it didn't start: …". */
export function notes(results: PluginAgentResult[]): string | null {
  const said = [...new Set(results.filter((result) => result.ok && result.message).map((result) => result.message!))];
  return said.length ? said.join("\n") : null;
}

/** Runs `action` and throws what the agents that didn't take it said. */
export async function checked(action: () => Promise<PluginAgentResult[]>): Promise<PluginAgentResult[]> {
  const results = await action();
  const problem = failures(results);
  if (problem) throw new Error(problem);
  return results;
}

/** Reads the page again every second while `on`. */
export function useWatch(refresh: () => void, on: boolean) {
  useEffect(() => {
    if (!on) return;
    const timer = setInterval(refresh, WATCH_MS);
    return () => clearInterval(timer);
  }, [refresh, on]);
}

/** Removes a Brigadier library item; detected originals have no Remove action. */
export function RemoveDialog({ item, adapter, onClose, onRemoved }: {
  item: PluginItemView;
  adapter: PluginsAdapter;
  onClose: () => void;
  /** Completion owns closing/navigation; it may outlive this dialog or its Settings visit. */
  onRemoved: () => void;
}) {
  const action = useAction();
  const remove = () => action.run(async () => {
    await checked(() => adapter.remove(item.key));
    onRemoved();
  });

  return (
    <PluginDialog
      slot="remove"
      onClose={onClose}
      title={`Remove ${item.name}?`}
      description="Removes it from Brigadier's library. Sessions already running keep it until they end."
    >
      <ErrorLine error={action.error} />
      <div className="flex justify-end gap-2">
        <Button variant="ghost" onClick={onClose}>Cancel</Button>
        <Button variant="destructive" disabled={action.busy} onClick={remove}>{action.busy ? "Removing…" : "Remove"}</Button>
      </div>
    </PluginDialog>
  );
}
