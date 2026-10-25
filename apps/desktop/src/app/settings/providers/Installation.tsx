import { useEffect, useRef } from "react";

import { useAction } from "@/app/conversation/useAction";
import { CLI_NAMES } from "@/app/settings/providers/AgentsOverview";
import { ROW_LINE, ROW_TITLE } from "@/app/settings/providers/parts";
import { ConnectStatus } from "@/app/setup/onboarding/ConnectStatus";
import { agentState } from "@/app/setup/onboarding/SetupTerminal";
import { Button, ErrorLine } from "@/components/controls";
import { SettingsCard } from "@/components/shared";
import { pickFile, request } from "@/ipc/client";
import type { ProviderKind, ProviderOverview } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { connectAgent, connecting, useConnectProgress } from "@/state/connect";
import { useAgentPageAsk } from "@/state/agentPage";
import { setSetting } from "@/state/settings";
import { useApp } from "@/state/store";
import { startUpdate, useUpdates } from "@/state/updates";

/** The Installation card: the CLI's version and its one action, then where it runs from. */
export function Installation({
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
    if (signIn && state !== "checking") useAgentPageAsk.setState({ ask: null });
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
      <Button variant="soft" size="sm" disabled={settingUp} onClick={() => void connectAgent(provider)}>
        Install
      </Button>
    );
  } else {
    const current = status?.version ? `v${status.version}` : "Version unknown";
    if (update?.progress.type === "updating") version = `${current} · updating to v${update.latest}…`;
    else if (update?.progress.type === "updated") version = `v${update.current} · updated`;
    else if (update) version = `${current} · v${update.latest} is out`;
    else version = `${current} · up to date`;
    if (state === "signIn") {
      action = (
        <Button variant="soft" size="sm" disabled={settingUp} onClick={() => void connectAgent(provider)}>
          Sign in
        </Button>
      );
    } else {
      // Always there, as drawn; it runs only an update Brigadier can run itself.
      const runnable = update?.action.type === "run" && update.progress.type !== "updated";
      const updating = update?.progress.type === "updating";
      action = (
        <Button
          variant="soft"
          size="sm"
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
        </Button>
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
          <Button variant="soft" size="sm" disabled={location.busy} onClick={change}>
            Change
          </Button>
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
