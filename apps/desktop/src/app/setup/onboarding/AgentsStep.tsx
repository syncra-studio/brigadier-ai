import { useEffect, useState } from "react";

import { PROVIDER_LABELS } from "@/app/inspector/providers/shared";
import { AgentCard } from "@/app/setup/onboarding/AgentCard";
import { StepFooter, StepHeader } from "@/app/setup/onboarding/parts";
import { type AgentState, agentState, checkAgents } from "@/app/setup/onboarding/SetupTerminal";
import type { ProviderKind, ProviderOverview } from "@/ipc/generated";
import { useApp } from "@/state/store";

// TODO(onboarding): more agents (Gemini CLI, OpenCode, Grok in the design) need detection and
// sign-in in the daemon first; ProviderKind holds only these two.
const PROVIDERS: readonly ProviderKind[] = ["claude", "codex"];

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
 * (the agent's own sign-in, in the browser; the card waits until it comes back) or Install (the
 * agent's own installer, then its sign-in, the card waiting meanwhile, as in Settings › Providers).
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

  // Installed some other way meanwhile (a terminal): it shows when the window comes back.
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
