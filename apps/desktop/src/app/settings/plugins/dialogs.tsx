import { useEffect, useState, type ReactNode } from "react";

import { useAction } from "@/app/conversation/useAction";
import { PROJECT_DIALOG_CARD, PROJECT_DIALOG_TITLE } from "@/app/shell/dialogs/fields";
import { Badge, Button, ErrorLine } from "@/components/controls";
import { Spinner, X } from "@/components/icons";
import { Dialog, DialogClose, DialogContent, DialogDescription, DialogTitle } from "@/components/overlays";
import type { PluginItemView, PluginsView } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { toast } from "@/state/toasts";
import type { PluginAgentResult, PluginsAdapter } from "./adapter";
import { has, listNames, labelOf, TAB_LABEL } from "./model";
import { AgentRow, stateOf } from "./parts";

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

/** The agents that didn't take a change, with why: "Codex: …". */
export function failures(view: PluginsView, results: PluginAgentResult[]): string | null {
  const failed = results.filter((result) => !result.ok);
  if (!failed.length) return null;
  return failed.map((result) => `${labelOf(view, result.agent)}: ${result.message ?? "it didn't take the change."}`).join("\n");
}

/** What the user should know although every agent took a change: "sentry is added, but it didn't start: …". */
export function notes(results: PluginAgentResult[]): string | null {
  const said = [...new Set(results.filter((result) => result.ok && result.message).map((result) => result.message!))];
  return said.length ? said.join("\n") : null;
}

/** Runs `action` and throws what the agents that didn't take it said. */
export async function checked(view: PluginsView, action: () => Promise<PluginAgentResult[]>): Promise<PluginAgentResult[]> {
  const results = await action();
  const problem = failures(view, results);
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

/** Who has an item now, for "Right now only … has it". */
const holders = (view: PluginsView, item: PluginItemView) =>
  item.foundIn ?? listNames(view.agents.filter((agent) => has(stateOf(item, agent.agent))).map((agent) => agent.label));

/** G6: puts an item that one agent (or another app) has into the Library, for every agent. Undo follows as a toast. */
export function UseEverywhereDialog({ view, item, adapter, onClose, onDone }: {
  view: PluginsView;
  item: PluginItemView;
  adapter: PluginsAdapter;
  onClose: () => void;
  /** After it is everywhere: a switch the user flipped waits for this. */
  onDone?: () => void;
}) {
  const action = useAction();
  const holder = holders(view, item);
  const kinds = TAB_LABEL[item.kind].toLowerCase().replace("mcp", "MCP");
  const confirm = () => action.run(async () => {
    // Undo takes the Library's copy it made, not the item it came from.
    let copy: string | null = null;
    await checked(view, async () => {
      const done = await adapter.everyAgent(item.key);
      copy = done.key;
      return done.results;
    });
    onClose();
    const undo = copy;
    toast(`${item.name} is in every agent now`, {
      actions: undo ? [{ label: "Undo", run: () => void adapter.undoEveryAgent(undo).catch((cause: unknown) => toast(String(cause), { tone: "error" })) }] : [],
    });
    onDone?.();
  });
  return (
    <PluginDialog slot="use-everywhere" onClose={onClose} title={`Use ${item.name} in every agent?`} description={`Right now only ${holder} has it. Brigadier gives it to every agent, the same way each agent adds its own ${kinds}. You can undo this.`}>
      <ol className="bg-foreground/5 rounded-control text-foreground/85 flex list-inside list-decimal flex-col gap-2 px-4 py-3 text-xs">
        <li>Keeps a copy in Brigadier's library</li>
        <li>Gives it to all {view.agents.length} agents</li>
        <li>{item.foundIn ? `Leaves ${item.foundIn}'s own copy as it is.` : `Removes ${holder}'s old copy, only after every agent has the new one.`}</li>
      </ol>
      <ErrorLine error={action.error} />
      <div className="flex justify-end gap-2">
        <Button variant="ghost" onClick={onClose}>Cancel</Button>
        <Button disabled={action.busy} onClick={confirm}>{action.busy ? "Working…" : "Use everywhere"}</Button>
      </div>
    </PluginDialog>
  );
}

/** G7: removes an item from every agent and every project. */
export function RemoveDialog({ view, item, adapter, onClose, onRemoved }: {
  view: PluginsView;
  item: PluginItemView;
  adapter: PluginsAdapter;
  onClose: () => void;
  /** Completion owns closing/navigation; it may outlive this dialog or its Settings visit. */
  onRemoved: () => void;
}) {
  const action = useAction();
  const signsIn = item.agents.some((entry) => entry.state.type === "needsSignIn") || (item.remote && item.kind === "server");
  const remove = () => action.run(async () => {
    await checked(view, () => adapter.remove(item.key));
    onRemoved();
  });
  return (
    <PluginDialog
      slot="remove"
      onClose={onClose}
      title={`Remove ${item.name}?`}
      description={`It goes from all ${view.agents.length} agents and every project${signsIn ? ", and each agent forgets its sign-in" : ""}. Sessions already running keep it until they end.`}
    >
      <ErrorLine error={action.error} />
      <div className="flex justify-end gap-2">
        <Button variant="ghost" onClick={onClose}>Cancel</Button>
        <Button variant="destructive" disabled={action.busy} onClick={remove}>{action.busy ? "Removing…" : "Remove"}</Button>
      </div>
    </PluginDialog>
  );
}

/** G5: each agent signs in to the item's server in turn; the rows follow each agent's own state. */
export function SignInDialog({ view, item, projectId, adapter, refresh, onClose }: {
  view: PluginsView;
  item: PluginItemView;
  projectId: string | null;
  adapter: PluginsAdapter;
  refresh: () => void;
  onClose: () => void;
}) {
  const [results, setResults] = useState<PluginAgentResult[]>([]);
  const [error, setError] = useState<string | null>(null);
  const start = () => {
    setError(null);
    if (!adapter.signIn) return;
    adapter.signIn(item.key, projectId).then(setResults, (cause: unknown) => setError(cause instanceof Error ? cause.message : String(cause)));
  };
  // Starts once, as the dialog opens.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  useEffect(start, []);
  const waiting = view.agents.filter((agent) => stateOf(item, agent.agent).type === "needsSignIn");
  useWatch(refresh, waiting.length > 0);
  const cancel = () => {
    void adapter.cancelSignIn?.(item.key).catch(() => {});
    onClose();
  };
  return (
    <PluginDialog slot="sign-in" onClose={cancel} title={`Sign in to ${item.name}`} description="Each agent keeps its own sign-in, so your browser asks once per agent. After the first one it usually just confirms.">
      <div className="flex flex-col gap-3">
        {view.agents.map((agent) => {
          const state = stateOf(item, agent.agent);
          const failed = results.find((result) => result.agent === agent.agent && !result.ok);
          const turn = waiting.indexOf(agent);
          if (failed) return <AgentRow key={agent.agent} agent={agent} tone="error" line={failed.message ?? "Couldn't sign in."} end={<span className="text-destructive text-xs">Failed</span>} />;
          if (state.type === "ready") return <AgentRow key={agent.agent} agent={agent} line="Signed in" end={<Badge variant="success">Signed in</Badge>} />;
          if (state.type === "notSupported") return <AgentRow key={agent.agent} agent={agent} dim line={state.reason} end={<Aside>Not supported</Aside>} />;
          if (turn === 0) return <AgentRow key={agent.agent} agent={agent} line="Waiting for your browser…" end={<Spinner aria-label="Waiting" className="text-foreground/65 size-icon-sm animate-spin" />} />;
          if (turn === 1) return <AgentRow key={agent.agent} agent={agent} line={`Opens your browser after ${waiting[0]!.label}`} end={<Aside>Next</Aside>} />;
          if (turn > 1) return <AgentRow key={agent.agent} agent={agent} line="Waiting" end={<Aside>Next</Aside>} />;
          return <AgentRow key={agent.agent} agent={agent} dim line={state.type === "wontStart" ? "Couldn't start it." : "Doesn't have it"} />;
        })}
      </div>
      <ErrorLine error={error} />
      <div className="flex items-center gap-2">
        {waiting.length > 0 && <Button variant="ghost" onClick={start}>Open browser again</Button>}
        <span className="flex-1" />
        {waiting.length > 0 ? <Button variant="ghost" onClick={cancel}>Cancel</Button> : <Button onClick={onClose}>Done</Button>}
      </div>
    </PluginDialog>
  );
}

const Aside = ({ children }: { children: ReactNode }) => <span className="text-foreground/50 shrink-0 text-xs">{children}</span>;
