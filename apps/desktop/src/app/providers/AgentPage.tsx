import { ChevronRight, InfoCircle } from "@openai/apps-sdk-ui/components/Icon";
import { type ReactNode, useEffect, useRef } from "react";

import { useAction } from "@/app/conversation/useAction";
import { ErrorLine } from "@/app/dialogs/fields";
import { ConnectStatus } from "@/app/onboarding/ConnectStatus";
import { agentState } from "@/app/onboarding/SetupTerminal";
import { ProviderAccounts, useRevealedEmails } from "@/app/providers/AccountRows";
import { CLI_NAMES } from "@/app/providers/AgentsOverview";
import { CheckAgainButton, PlanChip, ProvidersSection, ROW_LINE, ROW_TITLE, StateDot } from "@/app/providers/parts";
import { bestFor } from "@/app/routing/models";
import { SettingsButton, SettingsCard, SettingsPage, settingsRows, SettingsSwitch } from "@/app/settings/parts";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { pickFile, request } from "@/ipc/client";
import type { AccountView, MergedModel, ModelInfo, ProviderKind, ProviderOverview } from "@/ipc/generated";
import { formatAgo } from "@/lib/format";
import { isExpensive, TIER_LABELS, VENDOR_LABELS } from "@/lib/routing";
import { cn } from "@/lib/utils";
import { refreshProviders } from "@/state/actions";
import { refreshAccounts } from "@/state/accounts";
import { connectAgent, connecting, useConnectProgress } from "@/state/connect";
import { modelAvailable, providerOn, setModelAvailable, setProviderOn } from "@/state/providers";
import { setSetting } from "@/state/settings";
import { useApp } from "@/state/store";
import { startUpdate, useUpdates } from "@/state/updates";

/** Who makes each agent's CLI, on its page's line. */
const MAKERS: Record<ProviderKind, string> = { claude: "Claude Code by Anthropic", codex: "Codex CLI" };

/** The line under a model Brigadier gives no tasks to whatever the user chooses. */
const EXPENSIVE_NOTE =
  "Expensive: Brigadier uses it only where you pick it, in a chat, a session, your order or a rule";

/** What an agent switched off means, on its page's line. */
const OFF_NOTE = "Off: its models are hidden from the model picker, and it gets no work";

/**
 * An agent's Settings page: Providers to go back, its name and plan, its line, Enabled and Check
 * again; `children` under it.
 */
export function AgentSettingsPage({
  provider,
  overview,
  onBack,
  children,
}: {
  provider: ProviderKind;
  overview: ProviderOverview | undefined;
  onBack: () => void;
  children: ReactNode;
}) {
  const on = useApp((s) => providerOn(s.settings, provider));
  const toggle = useAction();
  const refresh = useAction();
  const state = agentState(overview);
  const label = VENDOR_LABELS[provider];
  const plan = overview?.status?.plan;
  const [tone, line] = !on
    ? (["off", OFF_NOTE] as const)
    : state === "ready"
      ? (["ready", `Ready · ${MAKERS[provider]}`] as const)
      : state === "checking"
        ? (["off", "Checking…"] as const)
        : (["needs", state === "install" ? "Not installed" : "Not signed in"] as const);
  return (
    <SettingsPage
      back={{ label: "Providers", onBack }}
      title={
        <span className="flex items-center gap-2.5">
          {label}
          {plan && <PlanChip plan={plan} large />}
        </span>
      }
      description={
        <span data-slot="agent-line" className="flex flex-col gap-1">
          <span className="flex min-w-0 items-center gap-1.75">
            <StateDot tone={tone} />
            <span className={cn("truncate", tone === "needs" && "text-warning")}>{line}</span>
          </span>
          <ErrorLine error={toggle.error ?? refresh.error} />
        </span>
      }
      actions={
        <>
          <span className="text-foreground/65 text-label flex items-center gap-2.5">
            Enabled
            <SettingsSwitch
              label={`Use ${label}`}
              checked={on}
              disabled={toggle.busy}
              onCheckedChange={(next) => toggle.run(() => setProviderOn(provider, next))}
            />
          </span>
          <CheckAgainButton
            busy={refresh.busy}
            aria-label={`Check ${label} and its accounts again`}
            onClick={() => refresh.run(() => Promise.all([refreshProviders(provider), refreshAccounts()]))}
          />
        </>
      }
    >
      {children}
    </SettingsPage>
  );
}

/**
 * An agent's page under its header: where its CLI is installed (its version with Update, which
 * runs when a newer one is out, Install or Sign in when it needs one, and where it runs from with Change),
 * its accounts with every limit and when each resets, and its models with a switch each.
 */
export function AgentPage({
  provider,
  overview,
  accounts,
  merged,
  now,
  signIn = false,
}: {
  provider: ProviderKind;
  overview: ProviderOverview | undefined;
  accounts: AccountView[] | null;
  merged: readonly MergedModel[] | null;
  now: number;
  /** Starts its sign-in when the page opens (the agents list's Sign in). */
  signIn?: boolean;
}) {
  const on = useApp((s) => providerOn(s.settings, provider));
  // Its own: going back to the agents hides the emails again.
  const revealed = useRevealedEmails();
  const checkedAtMs = overview?.checkedAtMs ?? null;
  return (
    <div data-slot="agent-page" className="flex flex-col gap-10">
      <ProvidersSection
        title="Installation"
        aside={checkedAtMs !== null ? `Checked ${formatAgo(checkedAtMs, now)}` : undefined}
      >
        <Installation provider={provider} overview={overview} signIn={signIn} />
      </ProvidersSection>
      <ProvidersSection title="Accounts" aside="Usage refreshes every few minutes">
        <SettingsCard>
          <ProviderAccounts page provider={provider} views={accounts} now={now} revealed={revealed} />
        </SettingsCard>
      </ProvidersSection>
      {on && <AgentModels provider={provider} overview={overview} merged={merged} />}
    </div>
  );
}

/** The Installation card: the CLI's version and its one action, then where it runs from. */
function Installation({
  provider,
  overview,
  signIn,
}: {
  provider: ProviderKind;
  overview: ProviderOverview | undefined;
  signIn: boolean;
}) {
  const state = agentState(overview);
  const status = overview?.status;
  const update = useUpdates((s) => s.view?.items.find((item) => item.target === provider));
  const updateError = useUpdates((s) => s.errors[provider] ?? null);
  const chosen = useApp((s) => s.settings.agentPaths.find((entry) => entry.provider === provider)?.path ?? null);
  const location = useAction();
  // Install and Sign in are Connect: the agent's own installer, then its own sign-in.
  const progress = useConnectProgress(provider);
  const settingUp = connecting(progress);
  // The agents list's Sign in starts it as the page opens, once.
  const started = useRef(false);
  useEffect(() => {
    if (!signIn || started.current || state !== "signIn") return;
    started.current = true;
    void connectAgent(provider);
  }, [signIn, state, provider]);

  let version: string;
  let action = null;
  if (state === "checking") version = "Checking…";
  else if (state === "install") {
    version = "Not installed";
    action = (
      <SettingsButton disabled={settingUp} onClick={() => void connectAgent(provider)}>
        Install
      </SettingsButton>
    );
  } else {
    const current = status?.version ? `v${status.version}` : "Version unknown";
    if (update?.progress.type === "updating") version = `${current} · updating to v${update.latest}…`;
    else if (update?.progress.type === "updated") version = `v${update.current} · updated`;
    else if (update) version = `${current} · v${update.latest} is out`;
    else version = `${current} · up to date`;
    if (state === "signIn") {
      action = (
        <SettingsButton disabled={settingUp} onClick={() => void connectAgent(provider)}>
          Sign in
        </SettingsButton>
      );
    } else {
      // Always there, as drawn; it runs only an update Brigadier can run itself.
      const runnable = update?.action.type === "run" && update.progress.type !== "updated";
      const updating = update?.progress.type === "updating";
      action = (
        <SettingsButton
          disabled={!runnable || updating}
          title={
            update?.action.type === "manual"
              ? `Update it with ${update.action.command}`
              : update?.action.type === "run"
                ? update.action.command
                : `${CLI_NAMES[provider]} is up to date`
          }
          onClick={() => update && void startUpdate(update)}
        >
          {updating ? "Updating…" : "Update"}
        </SettingsButton>
      );
    }
  }
  const failed = update?.progress.type === "failed" ? update.progress.error : updateError;

  const change = () =>
    location.run(async () => {
      const folder = (status?.path ?? chosen)?.replace(/\/[^/]*$/, "");
      const picked = await pickFile(`Choose the ${CLI_NAMES[provider]}`, folder);
      if (!picked) return;
      // Fails with what is wrong with it; only a real one is kept.
      await request({ method: "checkAgentPath", provider, path: picked });
      await saveChosen(provider, picked);
    });

  return (
    <SettingsCard>
      <div data-slot="agent-cli" className="flex flex-col px-4 py-3">
        <div className="flex items-center gap-6">
          <div className="flex min-w-0 flex-1 flex-col gap-1">
            <span className={ROW_TITLE}>{CLI_NAMES[provider]}</span>
            <span className={ROW_LINE}>{version}</span>
            <ErrorLine error={failed ?? null} />
          </div>
          {action}
        </div>
        {state !== "ready" && <ConnectStatus provider={provider} className="pt-3" />}
      </div>
      {state !== "install" && (
        <div data-slot="agent-location" className="flex items-center gap-6 px-4 py-3">
          <div className="flex min-w-0 flex-1 flex-col gap-1">
            <span className={ROW_TITLE}>Location</span>
            <span
              className={cn(ROW_LINE, "truncate font-mono")}
              title={status?.path ?? chosen ?? undefined}
            >
              {/* The one it runs: a chosen file that went away falls back to the one on PATH. */}
              {status?.path ?? chosen ?? "Checking…"}
            </span>
            <ErrorLine error={location.error} />
          </div>
          <SettingsButton disabled={location.busy} onClick={change}>
            Change
          </SettingsButton>
        </div>
      )}
    </SettingsCard>
  );
}

/** Saves `path` as the agent's binary. */
async function saveChosen(provider: ProviderKind, path: string): Promise<void> {
  const others = useApp.getState().settings.agentPaths.filter((entry) => entry.provider !== provider);
  await setSetting("agentPaths", [...others, { provider, path }]);
}

/** The Models section: a row each, a switch each; older models folded away. */
function AgentModels({
  provider,
  overview,
  merged,
}: {
  provider: ProviderKind;
  overview: ProviderOverview | undefined;
  merged: readonly MergedModel[] | null;
}) {
  const models = overview?.models?.models ?? [];
  const label = VENDOR_LABELS[provider];
  const state = agentState(overview);
  const current = models.filter((model) => !model.legacy);
  const older = models.filter((model) => model.legacy);
  const settings = useApp((s) => s.settings);
  const onCount = current.filter((model) => modelAvailable(settings, { provider, id: model.id })).length;
  const aside =
    models.length > 0 ? [`${onCount} on`, older.length > 0 && `${older.length} older`].filter(Boolean).join(" · ") : undefined;
  const row = (model: ModelInfo) => (
    <AgentModelRow key={model.id} provider={provider} model={model} merged={merged} />
  );
  return (
    <ProvidersSection title="Models" aside={aside}>
      <SettingsCard>
        {models.length === 0 ? (
          <p className="text-foreground/65 px-4 py-3 text-sm">
            {state === "checking"
              ? "Checking…"
              : state === "install"
                ? `Install ${label} to see its models.`
                : state === "signIn"
                  ? `Sign in to ${label} to see its models.`
                  : "Reading the models…"}
          </p>
        ) : (
          current.map(row)
        )}
        {older.length > 0 && (
          <Collapsible className={cn("flex flex-col", settingsRows)}>
            <CollapsibleTrigger
              className="text-foreground/65 hover:text-foreground text-label group flex w-full items-center gap-2.5 px-4 py-3"
            >
              <ChevronRight
                aria-hidden
                className="size-icon-sm transition-transform group-data-[state=open]:rotate-90 motion-reduce:transition-none"
              />
              Older models · {older.length}
            </CollapsibleTrigger>
            <CollapsibleContent className={cn("flex flex-col", settingsRows)}>
              {older.map(row)}
            </CollapsibleContent>
          </Collapsible>
        )}
      </SettingsCard>
    </ProvidersSection>
  );
}

/**
 * What a model's row shows and does: its switch, and its line (Expensive for a model Brigadier
 * never gives tasks, else how good it is and what for, else the agent's own words).
 */
function useModelRow(provider: ProviderKind, model: ModelInfo, merged: readonly MergedModel[] | null) {
  const ref = { provider, id: model.id };
  const available = useApp((s) => modelAvailable(s.settings, ref));
  const toggle = useAction();
  const known = merged?.find((entry) => entry.provider === provider && entry.id === model.id);
  const description =
    known && known.tier !== "unrated"
      ? [TIER_LABELS[known.tier], bestFor(known)].filter(Boolean).join(" · ")
      : model.description || null;
  return {
    available,
    toggle,
    expensive: isExpensive(model),
    description,
    setAvailable: (next: boolean) => toggle.run(() => setModelAvailable(ref, next)),
  };
}

/** A model on an agent's page: its name and id, its line, its switch. */
function AgentModelRow({
  provider,
  model,
  merged,
}: {
  provider: ProviderKind;
  model: ModelInfo;
  merged: readonly MergedModel[] | null;
}) {
  const { available, toggle, expensive, description, setAvailable } = useModelRow(provider, model, merged);
  const text = ROW_LINE;
  return (
    <div data-slot="provider-model" data-setting={model.displayName} className="flex items-center gap-6 px-4 py-3">
      <div className="flex min-w-0 flex-1 flex-col gap-1.25">
        <span className="flex min-w-0 flex-wrap items-center gap-x-2.5 gap-y-1">
          <span className={ROW_TITLE}>{model.displayName}</span>
          <span className="text-foreground/65 font-mono text-xs whitespace-pre">
            {model.id}
            {model.isDefault && "  ·  default"}
          </span>
        </span>
        {expensive ? (
          <span className={cn(text, "flex items-center gap-1.5")}>
            <InfoCircle aria-hidden className="size-icon-xs shrink-0" />
            {EXPENSIVE_NOTE}
          </span>
        ) : (
          description && <span className={text}>{description}</span>
        )}
        <ErrorLine error={toggle.error} />
      </div>
      <SettingsSwitch
        label={`Use ${model.displayName}`}
        checked={available}
        disabled={toggle.busy}
        onCheckedChange={setAvailable}
      />
    </div>
  );
}
