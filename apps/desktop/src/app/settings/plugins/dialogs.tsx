import { useEffect, useRef, useState, type ReactNode } from "react";

import { PROVIDER_LABELS } from "@/app/inspector/providers/shared";
import { useAction } from "@/app/conversation/useAction";
import { PROJECT_DIALOG_CARD, PROJECT_DIALOG_TITLE } from "@/app/shell/dialogs/fields";
import { Badge, Button, ErrorLine } from "@/components/controls";
import { Spinner, X } from "@/components/icons";
import { Dialog, DialogClose, DialogContent, DialogDescription, DialogTitle } from "@/components/overlays";
import { cn } from "@/lib/utils";
import type { PluginAgentResult, PluginsAdapter, SignInStep } from "./adapter";
import { AgentRow, type PluginAgent } from "./parts";
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

/**
 * Remove: takes out what Brigadier added. A Brigadier-added item goes from every agent; for one
 * an agent has of its own, only the copies Brigadier gave the others go, and its original stays.
 */
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
  const copies = item.removal === "copies";
  return (
    <PluginDialog
      slot="remove"
      onClose={onClose}
      title={`Remove ${item.name}?`}
      description={
        copies
          ? `Brigadier takes out only the copies it gave your other agents. Your own original stays as it is, so ${item.name} stays listed, and Share with every agent brings the copies back. Sessions already running keep it until they end.`
          : `It goes from every agent and every project. Sessions already running keep it until they end.`
      }
    >
      <ErrorLine error={action.error} />
      <div className="flex justify-end gap-2">
        <Button variant="ghost" onClick={onClose}>Cancel</Button>
        <Button variant="destructive" disabled={action.busy} onClick={remove}>{action.busy ? "Removing…" : "Remove"}</Button>
      </div>
    </PluginDialog>
  );
}

/** An agent's sign-in step, as its row says it: its line, its tone, and whether it's signed in. */
function stepRow(step: SignInStep): { line: string; tone: "muted" | "error"; done: boolean } {
  switch (step.state.type) {
    case "signedIn":
    case "alreadySignedIn":
      return { line: "Signed in", tone: "muted", done: true };
    case "failed":
      return { line: step.state.reason || "Couldn't sign in.", tone: "error", done: false };
    case "cancelled":
      return { line: "Cancelled.", tone: "muted", done: false };
    case "inBrowser":
      return { line: "Waiting for your browser…", tone: "muted", done: false };
    case "starting":
      return { line: "Opening your browser…", tone: "muted", done: false };
    case "waiting":
      return { line: "Waiting…", tone: "muted", done: false };
  }
}

/**
 * Sign in: every agent signs in to the server at once, each in its own browser tab. Each agent's
 * row shows its step as it happens; the page is read again as soon as one is signed in.
 */
export function SignInDialog({ item, serverId, projectId, adapter, refresh, onClose }: {
  item: PluginItemView;
  serverId: string;
  /** Where the sign-in runs (null: every project). */
  projectId: string | null;
  adapter: PluginsAdapter;
  refresh: () => void;
  onClose: () => void;
}) {
  const [results, setResults] = useState<PluginAgentResult[] | null>(null);
  const [steps, setSteps] = useState<SignInStep[]>([]);
  const [error, setError] = useState<string | null>(null);
  // How many agents were signed in at the last step heard: one more reads the page again.
  const signedIn = useRef(0);
  const progress = (next: SignInStep[]) => {
    setSteps(next);
    const done = next.filter((step) => stepRow(step).done).length;
    if (done > signedIn.current) refresh();
    signedIn.current = done;
  };
  const start = () => {
    setError(null);
    setResults(null);
    setSteps([]);
    signedIn.current = 0;
    if (!adapter.signIn) return;
    adapter.signIn(item.key, serverId, projectId, progress).then(
      (done) => {
        setResults(done);
        refresh();
      },
      (cause: unknown) => setError(cause instanceof Error ? cause.message : String(cause)),
    );
  };
  // Starts once, as the dialog opens.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  useEffect(start, []);
  const waiting = results === null && error === null;
  useWatch(refresh, waiting);
  const cancel = () => {
    void adapter.cancelSignIn?.(item.key).catch(() => {});
    onClose();
  };
  const failed = results?.filter((result) => !result.ok) ?? [];
  return (
    <PluginDialog slot="sign-in" onClose={waiting ? cancel : onClose} title={`Sign in to ${item.name}`} description="Each agent keeps its own sign-in, so your browser asks once per agent. After the first one it usually just confirms.">
      <div className="flex flex-col gap-3">
        {waiting && steps.length === 0 && (
          <div data-slot="sign-in-waiting" className="text-foreground/65 flex items-center gap-3 text-label">
            <Spinner aria-label="Waiting" className="size-icon-sm animate-spin" />
            Waiting for your browser…
          </div>
        )}
        {waiting && steps.map((step) => {
          const row = stepRow(step);
          return (
            <AgentRow
              key={step.agent}
              agent={agentOf(step.agent)}
              tone={row.tone}
              line={row.line}
              end={row.done
                ? <Badge variant="success">Signed in</Badge>
                : row.tone === "error"
                  ? <span className="text-destructive text-xs">Failed</span>
                  : <Spinner aria-label="Waiting" className="text-foreground/65 size-icon-sm animate-spin" />}
            />
          );
        })}
        {results?.map((result) => (
          <AgentRow
            key={result.agent}
            agent={agentOf(result.agent)}
            tone={result.ok ? "muted" : "error"}
            line={result.ok ? "Signed in" : result.message ?? "Couldn't sign in."}
            end={result.ok ? <Badge variant="success">Signed in</Badge> : <span className="text-destructive text-xs">Failed</span>}
          />
        ))}
      </div>
      <ErrorLine error={error} />
      <div className="flex items-center gap-2">
        {(waiting || failed.length > 0 || error) && <Button variant="ghost" onClick={start}>Open browser again</Button>}
        <span className="flex-1" />
        {waiting ? <Button variant="ghost" onClick={cancel}>Cancel</Button> : <Button onClick={onClose}>Done</Button>}
      </div>
    </PluginDialog>
  );
}
