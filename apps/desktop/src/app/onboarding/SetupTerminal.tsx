import { ExclamationMarkCircle } from "@openai/apps-sdk-ui/components/Icon";
import { lazy, Suspense, useCallback, useEffect, useRef, useState } from "react";

import { PROVIDER_LABELS } from "@/app/inspector/providers/shared";
import { Spinner } from "@/components/glyphs/spinner";
import { Button } from "@/components/ui/button";
import type { ProviderKind, ProviderOverview } from "@/ipc/generated";
import { loadProviders, refreshProviders } from "@/state/actions";
import { closeSetupTerminal, openSetupTerminal } from "@/state/onboarding";
import { useApp } from "@/state/store";

/*
 * Installing an agent or signing in to it, in a terminal row: the first-run setup and the
 * Providers page both use it.
 */

// The terminal (xterm) loads only when an agent needs installing or signing in.
const TerminalView = lazy(() =>
  import("@/app/conversation/TerminalView").then((module) => ({ default: module.TerminalView })),
);

/** How often an agent is checked again while its install or sign-in terminal is open. */
const RECHECK_MS = 3000;

export type AgentState = "checking" | "install" | "signIn" | "ready";

export function agentState(overview: ProviderOverview | undefined): AgentState {
  const status = overview?.status;
  if (!status) return "checking";
  if (!status.path) return "install";
  return status.loggedIn ? "ready" : "signIn";
}

/** Checks every agent again; results arrive as events. */
export async function checkAgents(): Promise<void> {
  try {
    // Events for a check apply only once the providers' view is loaded.
    if (!useApp.getState().providers.view) await loadProviders();
    await refreshProviders();
  } catch (error) {
    console.error("checking the agents failed", error);
  }
}

/** After its command succeeds, how long an agent may take to show as set up before we say so. */
const SETTLE_MS = 10_000;

/**
 * Runs the agent's installer or sign-in. Its output stays folded away unless asked for, or
 * the command fails; the row closes by itself once the agent is set up.
 */
export function SetupTerminal({
  provider,
  install,
  onClose,
}: {
  provider: ProviderKind;
  install: boolean;
  onClose: () => void;
}) {
  const terminalId = useRef<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  // Absent while the command runs; then its exit code.
  const [exit, setExit] = useState<{ code: number | null } | null>(null);
  const [stuck, setStuck] = useState(false);
  // Absent: the output shows only when the command failed.
  const [shown, setShown] = useState<boolean | null>(null);

  const open = useCallback(
    async (cols: number, rows: number) => {
      const terminal = await openSetupTerminal(provider, install, cols, rows);
      terminalId.current = terminal.id;
      return terminal;
    },
    [provider, install],
  );
  const onExit = useCallback(
    (code: number | null) => {
      terminalId.current = null;
      setExit({ code });
      void refreshProviders(provider).catch(() => {});
    },
    [provider],
  );

  // Notices the sign-in (or install) soon after it finishes.
  useEffect(() => {
    const timer = window.setInterval(() => {
      void refreshProviders(provider).catch(() => {});
    }, RECHECK_MS);
    return () => window.clearInterval(timer);
  }, [provider]);
  // Succeeded, but the agent still isn't set up a while later: the output may say why.
  useEffect(() => {
    if (exit?.code !== 0) return;
    const timer = window.setTimeout(() => setStuck(true), SETTLE_MS);
    return () => window.clearTimeout(timer);
  }, [exit]);
  // Leaving the step (or the dialog) stops the command.
  useEffect(
    () => () => {
      if (terminalId.current) closeSetupTerminal(terminalId.current);
    },
    [],
  );

  const label = PROVIDER_LABELS[provider];
  const failed = exit !== null && exit.code !== 0;
  const expanded = shown ?? (failed || stuck);
  const status =
    exit === null
      ? install
        ? `Installing ${label}…`
        : `Finish signing in to ${label} in your browser…`
      : failed
        ? `${install ? `${label} could not be installed` : `Signing in to ${label} failed`}${exit.code === null ? "" : ` (exit code ${exit.code})`}.`
        : stuck
          ? `${label} still isn't ready. The output may say why.`
          : `Checking ${label}…`;

  const cancel = () => {
    if (terminalId.current) closeSetupTerminal(terminalId.current);
    terminalId.current = null;
    onClose();
  };
  const retry = () => {
    setAttempt((current) => current + 1);
    setExit(null);
    setStuck(false);
    setShown(null);
  };

  return (
    <div className="rounded-surface min-w-0 overflow-hidden border">
      <div className="bg-muted/30 flex items-center gap-2 px-3 py-2">
        {failed || stuck ? (
          <ExclamationMarkCircle className="text-destructive size-icon-sm shrink-0" />
        ) : (
          <Spinner className="text-muted-foreground size-icon-sm shrink-0 animate-spin" />
        )}
        <span role="status" className="text-muted-foreground min-w-0 flex-1 truncate text-xs">
          {status}
        </span>
        <Button size="xs" variant="ghost" onClick={() => setShown(!expanded)}>
          {expanded ? "Hide output" : "Show output"}
        </Button>
        {(failed || stuck) && (
          <Button size="xs" variant="ghost" onClick={retry}>
            Try again
          </Button>
        )}
        <Button size="xs" variant="ghost" onClick={cancel}>
          {exit === null ? "Cancel" : "Close"}
        </Button>
      </div>
      {/* Folded, the terminal keeps its size (so the CLI's output wraps as it would) but no height. */}
      <div className={expanded ? "border-t" : "h-0 overflow-hidden"}>
        <Suspense fallback={null}>
          <TerminalView
            key={attempt}
            open={open}
            onExit={onExit}
            focus={false}
            className="h-64 flex-none"
          />
        </Suspense>
      </div>
    </div>
  );
}
