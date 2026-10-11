import { ArrowRight, Check, ExclamationMarkCircle, Reload } from "@openai/apps-sdk-ui/components/Icon";
import { useEffect, useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { PROVIDER_LABELS } from "@/app/inspector/providers/shared";
import { SectionLabel, StepFooter, StepHeader } from "@/app/onboarding/parts";
import { type AgentState, agentState, checkAgents, SetupTerminal } from "@/app/onboarding/SetupTerminal";
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
import { useApp } from "@/state/store";

const PROVIDERS: readonly ProviderKind[] = ["claude", "codex"];

/** Whose account each agent works with, in its column's line. */
const ACCOUNT_OF: Record<ProviderKind, string> = { claude: "Claude", codex: "OpenAI" };

/** The note under the step's buttons. */
const MORE_LATER = "You can connect more accounts anytime in Settings.";

/** An agent's own page: which one, and whether its install or sign-in should start at once. */
type Detail = { provider: ProviderKind; start: boolean };

function useAgents() {
  const providers = useApp((s) => s.providers.view?.providers);
  return PROVIDERS.map((provider) => {
    const overview = providers?.find((entry) => entry.provider === provider);
    return { provider, overview, state: agentState(overview) };
  });
}

/**
 * Connecting the agents: Claude Code and Codex side by side, each with its logo, how it stands
 * and the one button that gets it working (Install, Sign in) or, once it works, opens its page
 * with its accounts. While an agent's page is open it takes Escape (through `onEscape`), back to
 * both.
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
  const [detail, setDetail] = useState<Detail | null>(null);
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
    const agent = agents.find((entry) => entry.provider === detail.provider);
    return (
      <AgentPage
        key={detail.provider}
        provider={detail.provider}
        overview={agent?.overview}
        state={agent?.state ?? "checking"}
        start={detail.start}
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
          ? "Sign in to an agent you already use. Brigadier works through it with your own subscription."
          : "Looking for the agents on this computer…";

  return (
    <div className="onboarding-step flex min-h-0 flex-1 flex-col">
      <StepHeader
        eyebrow={name ? `Nice to meet you, ${name}` : null}
        title="Connect your agents"
        description={description}
      />
      <div className="flex min-h-0 flex-1 flex-col overflow-y-auto px-6">
        <div className="max-w-onboarding-agents mx-auto mt-14 mb-8 grid w-full grid-cols-2 gap-16">
          {agents.map(({ provider, overview, state }) => (
            <AgentColumn
              key={provider}
              provider={provider}
              overview={overview}
              state={state}
              onOpen={(start) => setDetail({ provider, start })}
            />
          ))}
        </div>
        {ready === 0 && allChecked && (
          <div className="bg-muted/25 rounded-surface max-w-onboarding-agents mx-auto flex w-full items-center gap-3 border px-4 py-3">
            <ExclamationMarkCircle className="text-warning size-icon-md shrink-0" />
            <p className="min-w-0 flex-1 text-sm">
              Sessions need at least one agent that is signed in. You can also set this up later.
            </p>
            <Button size="sm" variant="ghost" disabled={recheck.busy} onClick={() => recheck.run(checkAgents)}>
              <Reload />
              Check again
            </Button>
          </div>
        )}
      </div>
      <StepFooter onBack={onBack} primary={{ label: "Continue", onClick: onContinue }} note={MORE_LATER} />
    </div>
  );
}

/** An agent's logo on its large tile. */
function AgentMark({ provider, small = false }: { provider: ProviderKind; small?: boolean }) {
  return (
    <span
      className={cn(
        "bg-muted/40 grid shrink-0 place-items-center border",
        small ? "rounded-control size-10" : "size-agent-mark rounded-3xl",
      )}
    >
      <ProviderGlyph provider={provider} className={small ? "size-icon-lg" : "size-12"} />
    </span>
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
  /** Opens its page; `start`: begin its install or sign-in there at once. */
  onOpen: (start: boolean) => void;
}) {
  const label = PROVIDER_LABELS[provider];
  const ready = state === "ready";
  return (
    <section className="flex min-w-0 flex-col items-center text-center">
      <AgentMark provider={provider} />
      <h2 className="mt-5 text-xl font-medium">{label}</h2>
      <p className="text-muted-foreground mt-1.5 mb-6 text-sm">
        Work with your {ACCOUNT_OF[provider]} account.
      </p>
      <Button
        size="lg"
        disabled={state === "checking"}
        aria-label={ready ? `Manage ${label}` : undefined}
        onClick={() => onOpen(!ready)}
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
        ) : (
          <>
            {state === "install" ? `Install ${label}` : `Sign in to ${label}`}
            <ArrowRight />
          </>
        )}
      </Button>
      <output className="text-muted-foreground mt-2.5 min-h-4 truncate text-xs">
        {state === "checking" ? "" : providerStatusText(overview)}
      </output>
    </section>
  );
}

/**
 * One agent's page: how it stands (with Check again), its install or sign-in in a terminal
 * while it needs one, and once it works its accounts, where another can be added. Leaving it
 * cancels a sign-in still under way (ProviderAccounts removes an account that never signed in).
 */
function AgentPage({
  provider,
  overview,
  state,
  start,
  onBack,
}: {
  provider: ProviderKind;
  overview: ProviderOverview | undefined;
  state: AgentState;
  start: boolean;
  onBack: () => void;
}) {
  const label = PROVIDER_LABELS[provider];
  const accounts = useApp((s) => s.accounts?.accounts);
  const revealed = useRevealedEmails();
  const now = useNow(30_000);
  const recheck = useAction();
  const needs = state === "install" || state === "signIn";
  // The install or sign-in under way, and the state it began from: its terminal closes by
  // itself once the agent moves on.
  const [setup, setSetup] = useState<AgentState | null>(() => (start && needs ? state : null));
  const settingUp = setup !== null && setup === state;

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
              {settingUp ? (
                <SetupTerminal
                  key={state}
                  provider={provider}
                  install={state === "install"}
                  onClose={() => {
                    setSetup(null);
                    void checkAgents();
                  }}
                />
              ) : (
                <div className="bg-muted/25 rounded-surface flex items-center gap-3 border px-4 py-3">
                  <p className="min-w-0 flex-1 text-sm">
                    {state === "install"
                      ? `${label} isn't installed on this computer yet.`
                      : `${label} is installed but not signed in.`}
                  </p>
                  <Button size="sm" onClick={() => setSetup(state)}>
                    {state === "install" ? "Install" : "Sign in"}
                  </Button>
                </div>
              )}
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
