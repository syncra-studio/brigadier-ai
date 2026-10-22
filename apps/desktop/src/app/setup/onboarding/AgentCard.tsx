import { PROVIDER_LABELS } from "@/app/inspector/providers/shared";
import { ConnectCode, ConnectGuide, useNeedsGuide } from "@/app/setup/onboarding/ConnectStatus";
import { PILL_BUTTON, RAISED_BUTTON } from "@/app/setup/onboarding/parts";
import type { AgentState } from "@/app/setup/onboarding/SetupTerminal";
import { Check, ProviderGlyph, Spinner } from "@/components/icons";
import { Badge, Button } from "@/components/controls";
import type { ProviderKind, ProviderOverview } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { cancelConnect, connectAgent, connecting, useConnect, useConnectProgress } from "@/state/connect";

/** "Max plan" from the plan the agent reports. */
function planLine(plan: string | null | undefined): string {
  // TODO(onboarding): not every login reports its plan (an API key, some Codex logins); those
  // read "Signed in" until the agent's status carries one.
  if (!plan) return "Signed in";
  return `${plan.charAt(0).toUpperCase()}${plan.slice(1)} plan`;
}

export function AgentCard({
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
  // Connect couldn't install it at all: the card says what is missing in full, and links the
  // vendor's install guide.
  const guide = useNeedsGuide(provider) && !askError;
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
      className="bg-card rounded-settings flex min-w-0 flex-col gap-3 border p-4"
    >
      <div className="flex min-w-0 items-center gap-3.5">
        <span className="bg-muted rounded-control size-agent-mark grid shrink-0 place-items-center">
          <ProviderGlyph provider={provider} className="size-6.5" />
        </span>
        <div className="flex min-w-0 flex-1 flex-col gap-1">
          <p className="truncate text-base font-medium">{label}</p>
          <p
            data-slot="agent-line"
            role={error ? "alert" : undefined}
            className={cn(
              guide ? "text-balance" : "truncate",
              "text-xs",
              error ? "text-destructive" : ready ? "text-muted-foreground" : "text-muted-foreground/70",
            )}
            title={error ?? undefined}
          >
            {line}
          </p>
          {guide && <ConnectGuide provider={provider} />}
        </div>
        {ready ? (
          <Badge variant="success" className="bg-success/10 h-7 gap-1.5 px-2.5">
            <Check />
            Connected
          </Badge>
        ) : waiting ? (
          // Waiting… is the way out too: hovered or focused it reads Cancel, in the same room (the
          // wider of the two holds it), and stops the sign-in.
          <button
            type="button"
            data-slot="agent-wait"
            aria-label={`Cancel sign-in to ${label}`}
            onClick={() => void cancelConnect(provider)}
            className={cn(
              PILL_BUTTON,
              "group/wait text-muted-foreground hover:text-foreground focus-visible:text-foreground grid shrink-0 items-center px-1 font-medium transition-colors",
            )}
          >
            <span className="col-start-1 row-start-1 flex items-center gap-1.75 group-hover/wait:invisible group-focus-visible/wait:invisible">
              <Spinner className="size-3.25 animate-spin" />
              Waiting…
            </span>
            <span className="invisible col-start-1 row-start-1 text-center group-hover/wait:visible group-focus-visible/wait:visible">
              Cancel
            </span>
          </button>
        ) : state === "signIn" || state === "install" ? (
          // Both are Connect, as in Settings › Providers: Install runs the vendor's own installer
          // first, then its sign-in.
          <Button size="sm" variant="outline" className={RAISED_BUTTON} onClick={() => void connectAgent(provider)}>
            {state === "install" ? "Install" : "Sign in"}
          </Button>
        ) : null}
      </div>
      {/* The browser couldn't hand the sign-in back: the code its page shows goes in here. */}
      {waiting && <ConnectCode provider={provider} />}
    </li>
  );
}
