import { ArrowRight, Check, ExclamationMarkCircle, Reload } from "@openai/apps-sdk-ui/components/Icon";
import { useEffect, useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { PROVIDER_LABELS } from "@/app/inspector/providers/shared";
import { ConnectStatus } from "@/app/onboarding/ConnectStatus";
import { SectionLabel, StepFooter, StepHeader } from "@/app/onboarding/parts";
import { type AgentState, agentState, checkAgents } from "@/app/onboarding/SetupTerminal";
import { ProviderAccounts, useRevealedEmails } from "@/app/providers/AccountRows";
import { ProviderGlyph } from "@/components/glyphs/provider-glyphs";
import { Spinner } from "@/components/glyphs/spinner";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { useNow } from "@/hooks/use-now";
import type { ProviderKind, ProviderOverview } from "@/ipc/generated";
import { shortPath } from "@/lib/format";
import { providerStatusText } from "@/lib/setup";
import { cn } from "@/lib/utils";
import { accountsOf, loadAccounts, refreshAccounts } from "@/state/accounts";
import { connectAgent, connecting, useConnect, useConnectProgress } from "@/state/connect";
import { useApp } from "@/state/store";

const PROVIDERS: readonly ProviderKind[] = ["claude", "codex"];

/** The note under the step's buttons. */
const MORE_LATER = "You can connect more accounts anytime in Settings.";

function useAgents() {
  const providers = useApp((s) => s.providers.view?.providers);
  return PROVIDERS.map((provider) => {
    const overview = providers?.find((entry) => entry.provider === provider);
    return { provider, overview, state: agentState(overview) };
  });
}

/**
 * Connecting the agents: Claude Code and Codex side by side, each with its logo, Connected or
 * Not connected, and the one button that gets it working (Connect: Brigadier installs what is
 * missing and opens the agent's own sign-in) or, once it works, opens its page with its
 * accounts. While an agent's page is open it takes Escape (through `onEscape`), back to both.
 */
export function AgentsStep({
  name,
  onEscape,
  onBack,
  onContinue,
}: {
  name: string;
  /** Hands the setup what Escape does first (null: nothing of its own); stable. */
  onEscape: (handler: (() => boolean) | null) => void;
  onBack: () => void;
  onContinue: () => void;
}) {
  const connected = useApp((s) => s.connection.status === "connected");
  const agents = useAgents();
  const [detail, setDetail] = useState<ProviderKind | null>(null);
  const recheck = useAction();

  useEffect(() => {
    if (connected) void checkAgents();
  }, [connected]);

  useEffect(() => {
    if (!detail) return;
    onEscape(() => {
      setDetail(null);
      return true;
    });
    return () => onEscape(null);
  }, [detail, onEscape]);

  if (detail) {
    const agent = agents.find((entry) => entry.provider === detail);
    return (
      <AgentPage
        key={detail}
        provider={detail}
        overview={agent?.overview}
        state={agent?.state ?? "checking"}
        onBack={() => setDetail(null)}
      />
    );
  }

  const ready = agents.filter((agent) => agent.state === "ready").length;
  const allChecked = agents.every((agent) => agent.state !== "checking");
  const description =
    ready === agents.length
      ? "Both agents are ready. You can start right away."
      : ready > 0
        ? "You're connected and ready to go. Add the other agent anytime."
        : allChecked
          ? "Connect Claude Code or Codex with your own subscription. Brigadier sets up whatever is missing."
          : "Looking for the agents on this computer…";

  return (
    <div className="onboarding-step flex min-h-0 flex-1 flex-col">
      <StepHeader
        eyebrow={name ? `Nice to meet you, ${name}` : null}
        title="Connect your agents"
        description={description}
      />
      <div className="flex min-h-0 flex-1 flex-col overflow-y-auto px-6">
        <div className="max-w-onboarding-agents mx-auto mt-16 mb-8 grid w-full grid-cols-2 gap-16">
          {agents.map(({ provider, overview, state }) => (
            <AgentColumn
              key={provider}
              provider={provider}
              overview={overview}
              state={state}
              onOpen={() => setDetail(provider)}
            />
          ))}
        </div>
        {ready === 0 && allChecked && (
          <div className="bg-muted/25 rounded-surface max-w-onboarding-agents mx-auto flex w-full items-center gap-3 border px-4 py-3">
            <ExclamationMarkCircle className="text-warning size-icon-md shrink-0" />
            <p className="min-w-0 flex-1 text-sm">
              Connect at least one agent to continue: your work runs through it.
            </p>
            <Button size="sm" variant="ghost" disabled={recheck.busy} onClick={() => recheck.run(checkAgents)}>
              <Reload />
              Check again
            </Button>
          </div>
        )}
      </div>
      <StepFooter
        onBack={onBack}
        primary={{ label: "Continue", onClick: onContinue, disabled: ready === 0 }}
        note={MORE_LATER}
      />
    </div>
  );
}

/** An agent's logo, bare: large over its column, smaller at the head of its page. */
function AgentMark({ provider, small = false }: { provider: ProviderKind; small?: boolean }) {
  return (
    <ProviderGlyph provider={provider} className={cn("shrink-0", small ? "size-8" : "size-agent-mark")} />
  );
}

function AgentColumn({
  provider,
  overview,
  state,
  onOpen,
}: {
  provider: ProviderKind;
  overview: ProviderOverview | undefined;
  state: AgentState;
  /** Opens its page (once it is connected). */
  onOpen: () => void;
}) {
  const label = PROVIDER_LABELS[provider];
  const progress = useConnectProgress(provider);
  // Asking to connect failed (the daemon was away): said like a failed Connect.
  const askError = useConnect((s) => s.errors[provider]);
  const failed = progress?.type === "failed" || askError !== undefined;
  const ready = state === "ready";
  // Connect said Connected and the agent's check is on its way: still busy until it arrives.
  const busy = !ready && (connecting(progress) || progress?.type === "connected");
  const plan = overview?.status?.plan;
  return (
    <section data-slot="agent-column" className="flex min-w-0 flex-col items-center text-center">
      <AgentMark provider={provider} />
      <h2 className="mt-8 text-xl font-medium">{label}</h2>
      <p className="text-muted-foreground mt-1.5 mb-6 text-sm">Works with your own subscription.</p>
      <Button
        size="lg"
        disabled={state === "checking" || busy}
        aria-label={ready ? `Manage ${label}` : undefined}
        onClick={() => (ready ? onOpen() : void connectAgent(provider))}
        className={cn(
          "w-full gap-2",
          ready && "bg-success/15 text-success hover:bg-success/20",
        )}
      >
        {state === "checking" ? (
          <>
            <Spinner className="animate-spin" />
            Checking…
          </>
        ) : ready ? (
          <>
            Connected
            <Check />
          </>
        ) : busy ? (
          <>
            <Spinner className="animate-spin" />
            Connecting…
          </>
        ) : (
          <>
            {failed ? "Try again" : "Connect"}
            <ArrowRight />
          </>
        )}
      </Button>
      <div className="mt-2.5 flex min-h-8 w-full flex-col items-center">
        {busy || failed ? (
          <ConnectStatus provider={provider} retry={false} className="w-full justify-center text-start" />
        ) : (
          <output data-slot="agent-connection" className="text-muted-foreground truncate text-xs">
            {state === "checking"
              ? ""
              : ready
                ? plan
                  ? `Connected · ${plan.charAt(0).toUpperCase()}${plan.slice(1)} plan`
                  : "Connected"
                : "Not connected"}
          </output>
        )}
      </div>
    </section>
  );
}

/**
 * One agent's page: how it stands (with Check again), Connect while it needs installing or
 * signing in, and once it works its accounts, where another can be added. Leaving it cancels
 * an extra account's sign-in still under way (ProviderAccounts removes an account that never
 * signed in).
 */
function AgentPage({
  provider,
  overview,
  state,
  onBack,
}: {
  provider: ProviderKind;
  overview: ProviderOverview | undefined;
  state: AgentState;
  onBack: () => void;
}) {
  const label = PROVIDER_LABELS[provider];
  const accounts = useApp((s) => s.accounts?.accounts);
  const revealed = useRevealedEmails();
  const now = useNow(30_000);
  const recheck = useAction();
  const progress = useConnectProgress(provider);
  const needs = state === "install" || state === "signIn";

  useEffect(() => {
    loadAccounts().catch((error: unknown) => console.error("reading the accounts failed", error));
  }, []);

  const path = overview?.status?.path ?? null;
  return (
    <div className="onboarding-step flex min-h-0 flex-1 flex-col">
      <StepHeader
        title={`Connect ${label}`}
        description={`Brigadier works through ${label} on this computer, with your own subscription.`}
      />
      <div className="flex min-h-0 flex-1 flex-col overflow-y-auto px-6">
        <div className="max-w-onboarding-detail mx-auto mt-10 mb-6 flex w-full flex-col gap-6">
          <div className="bg-card/40 rounded-surface flex items-center gap-3 border p-4">
            <AgentMark provider={provider} small />
            <div className="min-w-0 flex-1">
              <p className="truncate text-base font-medium">{label}</p>
              <p className="text-muted-foreground mt-0.5 truncate font-mono text-2xs">
                {path ? shortPath(path) : "Not found on this computer"}
              </p>
            </div>
            {state === "checking" ? (
              <Spinner className="text-muted-foreground size-icon-sm animate-spin" />
            ) : (
              <Badge variant={state === "ready" ? "success" : "warning"}>{providerStatusText(overview)}</Badge>
            )}
            <Button
              size="icon-sm"
              variant="ghost"
              aria-label={`Check ${label} again`}
              disabled={recheck.busy}
              onClick={() => recheck.run(() => Promise.all([checkAgents(), refreshAccounts()]))}
            >
              <Reload />
            </Button>
          </div>
          {needs && (
            <section className="flex flex-col gap-3">
              <SectionLabel label="Get started" />
              <div className="bg-muted/25 rounded-surface flex flex-col gap-3 border px-4 py-3">
                <div className="flex items-center gap-3">
                  <p className="min-w-0 flex-1 text-sm">{label} isn't connected yet.</p>
                  <Button size="sm" disabled={connecting(progress)} onClick={() => void connectAgent(provider)}>
                    Connect
                  </Button>
                </div>
                <ConnectStatus provider={provider} retry={false} />
              </div>
            </section>
          )}
          {state === "ready" && (
            <section className="flex flex-col gap-3">
              <SectionLabel label="Accounts" />
              <ProviderAccounts
                provider={provider}
                views={accounts ? accountsOf(accounts, provider) : null}
                now={now}
                revealed={revealed}
              />
            </section>
          )}
        </div>
      </div>
      <StepFooter onBack={onBack} backLabel="Back to agents" note={MORE_LATER} />
    </div>
  );
}
