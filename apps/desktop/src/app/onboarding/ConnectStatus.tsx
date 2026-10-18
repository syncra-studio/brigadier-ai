import { ExclamationMarkCircle } from "@openai/apps-sdk-ui/components/Icon";

import { PROVIDER_LABELS } from "@/app/inspector/providers/shared";
import { Spinner } from "@/components/glyphs/spinner";
import { Button } from "@/components/ui/button";
import type { ProviderKind } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { cancelConnect, connectAgent, useConnect, useConnectProgress } from "@/state/connect";

/*
 * Connect's line for an agent while it installs or signs in (with Cancel), or why it failed
 * (with Try again): the first-run setup and the Providers page both show it. Nothing while
 * Connect isn't running for the agent.
 */

/** The line's words: busy (a spinner beside them) or failed; null when there is nothing to say. */
export function connectLine(
  provider: ProviderKind,
  progress: ReturnType<typeof useConnectProgress>,
  askError: string | undefined,
): { text: string; failed: boolean } | null {
  const label = PROVIDER_LABELS[provider];
  if (askError) return { text: askError, failed: true };
  switch (progress?.type) {
    case "installing":
      return { text: `Installing ${label}… This takes a minute or two.`, failed: false };
    case "signingIn":
      return { text: `Finish signing in to ${label} in your browser.`, failed: false };
    case "failed":
      return { text: progress.error, failed: true };
    default:
      return null;
  }
}

export function ConnectStatus({
  provider,
  retry = true,
  className,
}: {
  provider: ProviderKind;
  /** Offers Try again after a failure (off where the agent's own button retries). */
  retry?: boolean;
  className?: string;
}) {
  const progress = useConnectProgress(provider);
  const askError = useConnect((s) => s.errors[provider]);
  const line = connectLine(provider, progress, askError);
  if (!line) return null;
  return (
    <div data-slot="connect-status" className={cn("flex min-w-0 items-center gap-2", className)}>
      {line.failed ? (
        <ExclamationMarkCircle className="text-destructive size-icon-sm shrink-0" />
      ) : (
        <Spinner className="text-muted-foreground size-icon-sm shrink-0 animate-spin" />
      )}
      <span role="status" className="text-muted-foreground min-w-0 flex-1 text-xs text-balance">
        {line.text}
      </span>
      {line.failed
        ? retry && (
            <Button size="xs" variant="ghost" onClick={() => void connectAgent(provider)}>
              Try again
            </Button>
          )
        : (
            <Button size="xs" variant="ghost" onClick={() => void cancelConnect(provider)}>
              Cancel
            </Button>
          )}
    </div>
  );
}
