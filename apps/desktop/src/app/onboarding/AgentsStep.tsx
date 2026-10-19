import { ArrowUpRight, Check } from "@openai/apps-sdk-ui/components/Icon";
import { useEffect, useState } from "react";

import { PROVIDER_LABELS } from "@/app/inspector/providers/shared";
import { StepFooter, StepHeader } from "@/app/onboarding/parts";
import { type AgentState, agentState, checkAgents } from "@/app/onboarding/SetupTerminal";
import { ProviderGlyph } from "@/components/glyphs/provider-glyphs";
import { Spinner } from "@/components/glyphs/spinner";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { openUrl } from "@/ipc/client";
import type { ProviderKind, ProviderOverview } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { connectAgent, connecting, useConnect, useConnectProgress } from "@/state/connect";
import { useApp } from "@/state/store";

// TODO(onboarding): more agents (Gemini CLI, OpenCode, Grok in the design) need detection and
// sign-in in the daemon first; ProviderKind holds only these two.
const PROVIDERS: readonly ProviderKind[] = ["claude", "codex"];

// Install opens each vendor's own install page.
const INSTALL_GUIDES: Record<ProviderKind, string> = {
  claude: "https://code.claude.com/docs/en/setup",
  codex: "https://github.com/openai/codex",
};

/** The note under the cards. */
const MORE_LATER = "You can connect more accounts anytime in Settings.";

/** Connected first, then found on this computer, then the rest. */
const RANK: Record<AgentState, number> = { ready: 0, signIn: 1, install: 2, checking: 3 };

type Agent = { provider: ProviderKind; overview: ProviderOverview | undefined; state: AgentState };

function useAgents(): Agent[] {
  const providers = useApp((s) => s.providers.view?.providers);
  return PROVIDERS.map((provider) => {
    const overview = providers?.find((entry) => entry.provider === provider);
    return { provider, overview, state: agentState(overview) };
  });
}

/** "Claude Code", "Claude Code and Codex", "A, B and C". */
function listNames(names: readonly string[]): string {
  if (names.length <= 1) return names.join("");
  return `${names.slice(0, -1).join(", ")} and ${names.at(-1)}`;
}

/**
 * Connecting the agents: a card each, two to a row (one in a narrow window), connected ones
 * first, then those found on this computer, then the rest, in the order the first check found
 * them. Each card says how its agent stands and offers the one thing it needs next: Sign in
 * (the agent's own sign-in, in the browser; the card waits until it comes back) or Install (its
 * install guide, in the browser; the agents are checked again when the window comes back).
 * Only the cards scroll; Continue needs one connected agent.
 */
export function AgentsStep({ onBack, onContinue }: { onBack: () => void; onContinue: () => void }) {
  const connected = useApp((s) => s.connection.status === "connected");
  const mac = useApp((s) => s.info?.platform === "macos");
  const agents = useAgents();
  // The order is set once every agent has been checked, so cards don't jump as they connect.
  const [order, setOrder] = useState<readonly ProviderKind[] | null>(null);

  useEffect(() => {
    if (connected) void checkAgents();
  }, [connected]);

  // Back from an install guide: whatever was installed meanwhile shows.
  const missing = agents.some((agent) => agent.state === "install");
  useEffect(() => {
    if (!missing) return;
    const onFocus = () => void checkAgents();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [missing]);

  const allChecked = agents.every((agent) => agent.state !== "checking");
  if (allChecked && order === null) {
    setOrder(agents.toSorted((a, b) => RANK[a.state] - RANK[b.state]).map((agent) => agent.provider));
  }
  const sorted = order
    ? order.flatMap((provider) => agents.filter((agent) => agent.provider === provider))
    : agents;

  const ready = agents.filter((agent) => agent.state === "ready");
  const description = !allChecked && ready.length === 0
    ? `Looking for the agents on this ${mac ? "Mac" : "computer"}…`
    : ready.length === 0
      ? "Connect at least one agent to continue."
      : `${listNames(ready.map((agent) => PROVIDER_LABELS[agent.provider]))} ${ready.length === 1 ? "is" : "are"} ready. Connect any others you use, or start right away.`;

  return (
    <div className="onboarding-step flex min-h-0 flex-1 flex-col">
      <StepHeader title="Connect your agents" description={description} />
      <div className="mt-12 min-h-0 flex-1 overflow-y-auto px-6 pb-6">
        <div className="max-w-onboarding mx-auto flex w-full flex-col gap-3">
          <ul className="onboarding-agent-grid">
            {sorted.map(({ provider, overview, state }) => (
              <AgentCard key={provider} provider={provider} overview={overview} state={state} mac={mac} />
            ))}
          </ul>
          <p className="text-muted-foreground/70 text-xs">{MORE_LATER}</p>
        </div>
      </div>
      <StepFooter
        onBack={onBack}
        count={`${ready.length} of ${agents.length} connected`}
        primary={{ label: "Continue", onClick: onContinue, disabled: ready.length === 0 }}
      />
    </div>
  );
}

/** "Max plan" from the plan the agent reports. */
function planLine(plan: string | null | undefined): string {
  // TODO(onboarding): not every login reports its plan (an API key, some Codex logins); those
  // read "Signed in" until the agent's status carries one.
  if (!plan) return "Signed in";
  return `${plan.charAt(0).toUpperCase()}${plan.slice(1)} plan`;
}

function AgentCard({
  provider,
  overview,
  state,
  mac,
}: {
  provider: ProviderKind;
  overview: ProviderOverview | undefined;
  state: AgentState;
  mac: boolean;
}) {
  const label = PROVIDER_LABELS[provider];
  const progress = useConnectProgress(provider);
  // Asking to connect failed (the daemon was away): said like a failed Connect.
  const askError = useConnect((s) => s.errors[provider]);
  const error = askError ?? (progress?.type === "failed" ? progress.error : null);
  const ready = state === "ready";
  // Signing in, or Connected said and the agent's check on its way: the card waits.
  const waiting = !ready && (connecting(progress) || progress?.type === "connected");
  const here = mac ? "this Mac" : "this computer";

  const line = error
    ? error
    : waiting
      ? progress?.type === "installing"
        ? `Installing ${label}…`
        : "Complete sign-in in your browser"
      : state === "checking"
        ? "Checking…"
        : ready
          ? planLine(overview?.status?.plan)
          : state === "signIn"
            ? `Found on ${here} · Not signed in`
            : "Not installed";

  return (
    <li
      data-slot="agent-card"
      data-state={waiting ? "waiting" : state}
      className="bg-card rounded-surface flex min-w-0 items-center gap-3.5 border p-4"
    >
      <span className="bg-muted rounded-control size-agent-mark grid shrink-0 place-items-center">
        <ProviderGlyph provider={provider} className="size-6.5" />
      </span>
      <div className="flex min-w-0 flex-1 flex-col gap-1">
        <p className="truncate text-base font-medium">{label}</p>
        <p
          data-slot="agent-line"
          role={error ? "alert" : undefined}
          className={cn(
            "truncate text-xs",
            error ? "text-destructive" : ready ? "text-muted-foreground" : "text-muted-foreground/70",
          )}
          title={error ?? undefined}
        >
          {line}
        </p>
      </div>
      {ready ? (
        <Badge variant="success" className="h-control-sm px-2.5">
          <Check />
          Connected
        </Badge>
      ) : waiting ? (
        <span role="status" className="text-muted-foreground flex shrink-0 items-center gap-1.5 px-1 text-sm font-medium">
          <Spinner className="size-icon-sm animate-spin" />
          Waiting…
        </span>
      ) : state === "signIn" ? (
        <Button size="sm" variant="outline" onClick={() => void connectAgent(provider)}>
          Sign in
        </Button>
      ) : state === "install" ? (
        <Button
          size="sm"
          variant="outline"
          onClick={() => openUrl(INSTALL_GUIDES[provider]).catch((cause: unknown) => console.error(cause))}
        >
          Install
          <ArrowUpRight className="text-muted-foreground size-icon-xs" />
        </Button>
      ) : null}
    </li>
  );
}
