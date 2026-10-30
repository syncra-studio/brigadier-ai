/**
 * Startup waits for the agent CLIs: the startup screen stays until each one has finished its
 * login check and, when signed in, reported its model list (the saved one or a live one) or
 * failed to. A missing or signed-out CLI doesn't hold it up, nor does one the user switched off
 * or whose models were all made unavailable, and nothing holds it for longer than
 * `AGENTS_WAIT_MS` once the app has drawn underneath; after that the app's own "checking"
 * states take over.
 */
import type { ProviderOverview, Settings } from "@/ipc/generated";
import { setSplashStatus } from "@/lib/splash";
import { loadProviders } from "@/state/actions";
import { modelAvailable, providerOn } from "@/state/providers";
import { useApp } from "@/state/store";
import { cliFacts } from "@/lib/providerKinds";

/** The longest the startup screen waits for the agent CLIs. */
const AGENTS_WAIT_MS = 3_000;
/** A short wait says nothing; a longer one says what it is waiting for. */
const STATUS_AFTER_MS = 600;
/** The providers are read again this often while waiting, so a check that finished before
 * they were first loaded is not missed. */
const RELOAD_MS = 500;

/** The part of the settings that says which agents startup can skip. */
export type AgentChoices = Pick<Settings, "disabledProviders" | "hiddenModels" | "knownModels">;

/** The user left the agent nothing to run: it's switched off, or every model it has listed
 * before is made unavailable. */
function nothingToRun(settings: AgentChoices, overview: ProviderOverview): boolean {
  if (!providerOn(settings, overview.provider)) return true;
  const known = settings.knownModels.filter((model) => model.provider === overview.provider);
  return known.length > 0 && !known.some((model) => modelAvailable(settings, model));
}

/** Whether an agent CLI has said all startup needs: installed and signed in or not, and if so
 * its models. An agent with nothing to run isn't waited for. */
export function agentReady(overview: ProviderOverview, settings: AgentChoices): boolean {
  if (nothingToRun(settings, overview)) return true;
  const status = overview.status;
  if (!status) return false;
  if (!status.path || !status.loggedIn) return true;
  return overview.models !== null || overview.error !== null;
}

/** "Starting Claude Code…", "Starting Claude Code and Codex…". */
function waitingFor(pending: readonly ProviderOverview[]): string {
  const names = pending.map((overview) => cliFacts(overview.provider).label);
  const last = names.pop();
  return `Starting ${names.length > 0 ? `${names.join(", ")} and ${last}` : last}…`;
}

function reload(): void {
  void loadProviders().catch((error: unknown) => {
    console.error("loading providers failed", error);
  });
}

/** Resolves once every agent CLI is ready, or after `AGENTS_WAIT_MS`. */
export function waitForAgents(): Promise<void> {
  return new Promise((resolve) => {
    let saying = false;
    const check = () => {
      const { providers, settings } = useApp.getState();
      if (!providers.view) return;
      const pending = providers.view.providers.filter((overview) => !agentReady(overview, settings));
      if (pending.length === 0) done();
      else if (saying) setSplashStatus(waitingFor(pending));
    };
    const unsubscribe = useApp.subscribe((state, previous) => {
      if (state.providers.view !== previous.providers.view || state.settings !== previous.settings)
        check();
    });
    const reloading = setInterval(reload, RELOAD_MS);
    const status = setTimeout(() => {
      saying = true;
      check();
    }, STATUS_AFTER_MS);
    const cap = setTimeout(done, AGENTS_WAIT_MS);
    function done() {
      unsubscribe();
      clearInterval(reloading);
      clearTimeout(status);
      clearTimeout(cap);
      resolve();
    }
    reload();
    check();
  });
}
