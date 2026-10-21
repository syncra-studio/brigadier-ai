import { useEffect, useRef, useState, type ComponentType, type SVGProps } from "react";

import { Button } from "@/components/controls";
import { Clock, ExclamationMarkCircle, Info, Key, Reload, ShieldExclamation, Spinner } from "@/components/icons";
import { SettingsCard } from "@/components/shared";
import { cn } from "@/lib/utils";
import { useApp } from "@/state/store";
import { useIntegrations, type IntegrationAdapter } from "./adapter";
import { CommandChip, IntegrationSection, WorkerRow } from "./parts";
import type { IntegrationAgent, IntegrationServer, IntegrationSignIn } from "./types";

/*
 * A remote server's Sign-in section (settings.pen › Plugins · Signing in): one status row
 * saying whether workers can use it and what to do (#1107), then a row per agent saying where
 * its sign-in comes from (#1108). It never shows a token: the daemon never sends one.
 */

export const SIGN_IN_NOTE = "A sign-in made in Brigadier is used from the next time a session starts or resumes, not in one already mid-turn.";
/** While a sign-in waits for the browser, how often to ask how it went. */
const STATUS_MS = 1000;
const AGENTS = ["claude", "codex"] as const;
const NAME: Record<IntegrationAgent, string> = { claude: "Claude Code", codex: "Codex" };
const SHORT: Record<IntegrationAgent, string> = { claude: "Claude", codex: "Codex" };
const REFUSED = "The server didn't accept the sign-in.";

type Action = "signIn" | "tryAgain" | "reopen" | "cancel" | "signOut" | "askAgain" | "checkNow";
type Status = {
  tone: "success" | "warning" | "failed" | "muted";
  icon: ComponentType<SVGProps<SVGSVGElement>> | "spinner";
  title: string;
  line: string;
  actions: Action[];
};
type Row = { agent: IntegrationAgent; line: string } & (
  | { badge: string; tone: "success" | "secondary" | "warning" }
  | { command: string }
  | { waiting: true }
  | Record<never, never>
);

const usable = (state: IntegrationSignIn | undefined) => state?.kind === "native" || state?.kind === "reusable" || state?.kind === "brigadier";
const names = (agents: IntegrationAgent[]) => agents.map((agent) => NAME[agent]).join(" and ");
const day = (ms: number) => new Intl.DateTimeFormat("en-GB", { day: "numeric", month: "short" }).format(new Date(ms));

/** True when no worker can use the server until someone signs in: its tile says so. */
export function needsSignIn(server: IntegrationServer): boolean {
  const states = AGENTS.map((agent) => server.signIn[agent]).filter((state) => state !== undefined);
  return !server.error && !server.pendingSignIn && states.some((state) => state.kind === "needed") && !states.some(usable);
}

function loginName(name: string): string {
  return /^[\w.-]+$/.test(name) ? name : `'${name.replaceAll("'", "'\\''")}'`;
}

/** Each agent's row: an agent's own way to sign in only when that agent has the server in its own setup. */
function row(server: IntegrationServer, agent: IntegrationAgent, mac: boolean, waiting: boolean): Row {
  const state = server.signIn[agent];
  if (!mac || state?.kind === "unsupported") return { agent, line: "Brigadier can't read saved sign-ins on this system", badge: "Unknown", tone: "secondary" };
  if (waiting && !usable(state)) return { agent, line: "Will use the sign-in you're making in Brigadier", waiting: true };
  switch (state?.kind) {
    case "native": return { agent, line: `Signed in through ${NAME[agent]}'s own setup`, badge: "Signed in", tone: "success" };
    case "reusable": return { agent, line: `Uses the sign-in from ${NAME[state.from]}. No extra sign-in needed.`, badge: `Uses ${NAME[state.from]}'s`, tone: "secondary" };
    case "brigadier": return { agent, line: "Uses the sign-in you made in Brigadier", badge: "Uses Brigadier's", tone: "secondary" };
    case "notAllowed": return { agent, line: `Can't reuse ${NAME[state.from]}'s sign-in: macOS wasn't allowed to let Brigadier read it`, badge: "Not allowed", tone: "warning" };
    case "unknown": return { agent, line: "Not checked yet. Checking may make macOS ask whether Brigadier may read a saved sign-in.", badge: "Unknown", tone: "secondary" };
    default: {
      if (!server.nativeAgents.includes(agent)) return { agent, line: "Not signed in" };
      return agent === "claude"
        ? { agent, line: `Not signed in. Or type this inside Claude Code, then pick ${server.name}:`, command: "/mcp" }
        : { agent, line: "Not signed in. Or sign in for Codex yourself, in a terminal:", command: `codex mcp login ${loginName(server.name)}` };
    }
  }
}

/** The status row, from the agents' states, a sign-in waiting for the browser or the last one's failure. */
function status(server: IntegrationServer, mac: boolean, waiting: boolean, failed: string | null): Status {
  const agents = AGENTS.filter((agent) => server.signIn[agent]);
  const states = agents.map((agent) => server.signIn[agent]!);
  if (!mac) return { tone: "muted", icon: Info, title: "Can't check sign-ins on this system", line: "Brigadier reads saved sign-ins and signs in only on macOS for now. Each agent's own sign-in still works for its own workers.", actions: [] };
  if (waiting) return { tone: "muted", icon: "spinner", title: "Waiting for you in the browser", line: `Finish signing in to ${server.name} on the page that opened in your browser. Brigadier waits up to 5 minutes.`, actions: ["reopen", "cancel"] };
  if (failed) {
    const timedOut = failed.startsWith("Timed out");
    return { tone: "failed", icon: timedOut ? Clock : ExclamationMarkCircle, title: "Sign-in didn't work", line: failed === REFUSED ? `${failed} Check you signed in to the right workspace, then try again.` : failed, actions: ["tryAgain"] };
  }
  const able = agents.filter((agent) => usable(server.signIn[agent]));
  const others = agents.filter((agent) => !usable(server.signIn[agent]));
  const only = able.length === 1 ? able[0] : undefined;
  const refused = agents.find((agent) => server.signIn[agent]?.kind === "notAllowed");
  if (refused) {
    const from = (server.signIn[refused] as { from: IntegrationAgent }).from;
    return { tone: "warning", icon: ShieldExclamation, title: `Only ${SHORT[from]} workers can use it`, line: `macOS asked whether Brigadier may read ${NAME[from]}'s saved sign-in, and it wasn't allowed. ${NAME[refused]} can't reuse it until you allow it — or sign in once here instead.`, actions: ["askAgain", "signIn"] };
  }
  if (!others.length) {
    const brigadier = agents.filter((agent) => server.signIn[agent]?.kind === "brigadier");
    if (brigadier.length) {
      const since = states.find((state): state is { kind: "brigadier"; sinceMs?: number } => state.kind === "brigadier" && state.sinceMs !== undefined)?.sinceMs;
      return { tone: "success", icon: Key, title: "Every worker can use it", line: `Signed in once in Brigadier${since === undefined ? "" : ` on ${day(since)}`}. That sign-in serves ${names(brigadier)} workers.`, actions: ["signOut"] };
    }
    const reusers = agents.filter((agent) => server.signIn[agent]?.kind === "reusable");
    const sources = AGENTS.filter((agent) => server.signIn[agent]?.kind === "native" || reusers.some((other) => (server.signIn[other] as { from: IntegrationAgent }).from === agent));
    return { tone: "success", icon: Key, title: "Every worker can use it", line: reusers.length ? `Signed in through ${names(sources)}. Brigadier reuses that sign-in for ${names(reusers)} workers, so every worker can use it.` : `Signed in through ${names(sources)}, so every worker can use it.`, actions: [] };
  }
  const unknown = others.filter((agent) => server.signIn[agent]?.kind === "unknown");
  if (only && unknown.length) return { tone: "warning", icon: Key, title: `Only ${SHORT[only]} workers can use it for now`, line: `Brigadier hasn't checked yet whether ${unknown.map((agent) => SHORT[agent]).join(" and ")} workers can reuse a sign-in. Check now, or sign in once here.`, actions: ["checkNow", "signIn"] };
  if (only) {
    const other = others[0]!;
    return { tone: "warning", icon: Key, title: `Only ${SHORT[only]} workers can use it`, line: `${NAME[other]} has no sign-in Brigadier can reuse, so ${SHORT[other]} workers can't use it yet. Sign in once here and it serves every agent's workers.`, actions: ["signIn"] };
  }
  if (unknown.length) return { tone: "warning", icon: Key, title: "Not checked yet", line: "Brigadier hasn't checked yet whether workers can reuse a sign-in. Check now, or sign in once here.", actions: ["checkNow", "signIn"] };
  return { tone: "warning", icon: Key, title: "Needs a sign-in", line: "No agent has a sign-in Brigadier can reuse, so no worker can use it yet. Sign in once here and it serves every agent's workers.", actions: ["signIn"] };
}

const TILE: Record<Status["tone"], string> = {
  success: "bg-success/15 text-success",
  warning: "bg-warning/15 text-warning",
  failed: "bg-destructive/10 text-destructive",
  muted: "bg-foreground/5 text-foreground/65",
};

export function SignInSection({ server, adapter, run, focus = false }: {
  server: IntegrationServer; adapter: IntegrationAdapter | null; run: (action: () => Promise<unknown>) => void; focus?: boolean | undefined;
}) {
  const mac = useApp((state) => state.info?.platform === "macos");
  const ref = useRef<HTMLDivElement>(null);
  // The attempt this page started; the ids already finished, so a snapshot read before the
  // daemon let one go doesn't bring it back; the last attempt's failure.
  const [attempt, setAttempt] = useState<string | null>(null);
  const [done, setDone] = useState<string[]>([]);
  const [failed, setFailed] = useState<string | null>(null);
  const restored = server.pendingSignIn && !done.includes(server.pendingSignIn) ? server.pendingSignIn : null;
  const pending = mac ? attempt ?? restored : null;

  useEffect(() => { if (focus) ref.current?.scrollIntoView({ block: "start" }); }, [focus]);
  useEffect(() => {
    if (!pending || !adapter) return;
    let live = true;
    const timer = setInterval(() => void adapter.signInStatus(pending).then((result) => {
      if (!live || result.type === "pending") return;
      finish(pending, result.type === "failed" ? result.reason : null);
    }, () => {}), STATUS_MS);
    return () => { live = false; clearInterval(timer); };
  }, [pending, adapter]);

  function finish(id: string, reason: string | null) {
    setDone((ids) => [...ids, id]);
    setAttempt(null);
    setFailed(reason);
    useIntegrations.getState().refresh();
  }
  const act = (action: Action) => {
    if (!adapter) return;
    switch (action) {
      case "signIn":
      case "tryAgain": return run(async () => { setFailed(null); setAttempt(await adapter.signIn(server.id)); });
      case "reopen": return pending && run(() => adapter.reopenSignIn(pending));
      case "cancel": return pending && run(async () => { await adapter.cancelSignIn(pending); finish(pending, null); });
      case "signOut": return run(async () => { await adapter.signOut(server.id); useIntegrations.getState().refresh(); });
      case "askAgain":
      case "checkNow": return run(async () => { await adapter.askSignInAgain(server.id); useIntegrations.getState().refresh(); });
    }
  };

  const shown = status(server, mac, !!pending, failed);
  const Icon = shown.icon;
  return <div ref={ref} className="scroll-mt-4">
    <IntegrationSection title="Sign-in" description="Each agent keeps its own sign-in. Brigadier reuses one from whichever agent has it, and never shows a token." aside="macOS only for now"><SettingsCard>
      <div data-slot="sign-in-status" className="flex items-center gap-3 px-4 py-3">
        <span className={cn("rounded-control flex size-8 shrink-0 items-center justify-center", TILE[shown.tone])}>{Icon === "spinner" ? <Spinner className="text-foreground size-4 animate-spin motion-reduce:animate-none" /> : <Icon className="size-4" />}</span>
        <div className="flex min-w-0 flex-1 flex-col gap-0.75"><span className={cn("text-label font-medium", shown.tone === "failed" && "text-destructive")}>{shown.title}</span><span className="text-foreground/65 text-xs leading-integration">{shown.line}</span></div>
        {shown.actions.length > 0 && <div className="flex shrink-0 items-center gap-2">{shown.actions.map((action) => <ActionButton key={action} action={action} disabled={!adapter} onClick={() => act(action)} />)}</div>}
      </div>
      {AGENTS.filter((agent) => server.signIn[agent] || !mac).map((agent) => {
        const item = row(server, agent, mac, !!pending);
        return <WorkerRow key={agent} agent={agent} title={NAME[agent]} line={item.line} {...("badge" in item ? { status: item.badge, tone: item.tone } : {})} trailing={"command" in item ? <CommandChip command={item.command} /> : "waiting" in item ? <span className="text-foreground/65 flex shrink-0 items-center gap-1.5 text-xs"><Spinner className="text-foreground size-3 animate-spin motion-reduce:animate-none" />Waiting</span> : undefined} />;
      })}
    </SettingsCard></IntegrationSection>
  </div>;
}

function ActionButton({ action, disabled, onClick }: { action: Action; disabled: boolean; onClick: () => void }) {
  switch (action) {
    case "signIn": return <Button size="sm" disabled={disabled} onClick={onClick}>Sign in in Brigadier</Button>;
    case "tryAgain": return <Button variant="soft" size="sm" disabled={disabled} onClick={onClick}><Reload className="size-3.5" />Try again</Button>;
    case "reopen": return <Button variant="ghost" size="sm" disabled={disabled} onClick={onClick}>Open the page again</Button>;
    case "cancel": return <Button variant="soft" size="sm" disabled={disabled} onClick={onClick}>Cancel</Button>;
    case "signOut": return <Button variant="ghost" size="sm" disabled={disabled} onClick={onClick}>Sign out</Button>;
    case "askAgain": return <Button variant="soft" size="sm" disabled={disabled} onClick={onClick}>Ask macOS again</Button>;
    case "checkNow": return <Button variant="soft" size="sm" disabled={disabled} onClick={onClick}>Check now</Button>;
  }
}
