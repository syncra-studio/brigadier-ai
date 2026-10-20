import { ExclamationMarkCircle } from "@openai/apps-sdk-ui/components/Icon";
import { type FormEvent, useState } from "react";

import { errorText } from "@/app/dialogs/fields";
import { PROVIDER_LABELS } from "@/app/inspector/providers/shared";
import { Spinner } from "@/components/glyphs/spinner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { openUrl } from "@/ipc/client";
import type { ProviderKind } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import {
  cancelConnect,
  connectAgent,
  sendConnectCode,
  useConnect,
  useConnectCodePage,
  useConnectProgress,
} from "@/state/connect";

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
    <div className={cn("flex min-w-0 flex-col gap-2", className)}>
      <div data-slot="connect-status" className="flex min-w-0 items-center gap-2">
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
      <ConnectCode provider={provider} />
    </div>
  );
}

/**
 * While an agent's sign-in offers a code page (Claude Code's does): for when the browser can't
 * hand the sign-in back by itself, a link to the page that shows a code, and a field to paste
 * that code into. Nothing otherwise.
 */
export function ConnectCode({ provider, className }: { provider: ProviderKind; className?: string }) {
  const page = useConnectCodePage(provider);
  const signingIn = useConnectProgress(provider)?.type === "signingIn";
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  if (!page || !signingIn) return null;

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (!code.trim() || busy) return;
    setBusy(true);
    setError(null);
    try {
      await sendConnectCode(provider, code);
      setCode("");
    } catch (cause) {
      setError(errorText(cause));
    } finally {
      setBusy(false);
    }
  };

  return (
    <form
      data-slot="connect-code"
      data-own-enter
      onSubmit={(event) => void submit(event)}
      className={cn("flex min-w-0 flex-col gap-1.5", className)}
    >
      <p className="text-muted-foreground text-xs">
        Browser didn't finish?{" "}
        <button
          type="button"
          className="text-link hover:underline"
          onClick={() => openUrl(page).catch((cause: unknown) => console.error(cause))}
        >
          Open the code page
        </button>
        , then paste the code it shows.
      </p>
      <div className="flex min-w-0 items-center gap-2">
        <Input
          aria-label={`Code for ${PROVIDER_LABELS[provider]}`}
          placeholder="Paste the code from the page"
          autoComplete="off"
          spellCheck={false}
          value={code}
          disabled={busy}
          aria-invalid={error !== null || undefined}
          onChange={(event) => setCode(event.target.value)}
          className="text-xs"
        />
        <Button type="submit" size="sm" variant="outline" disabled={busy || !code.trim()}>
          Send
        </Button>
      </div>
      {error && (
        <p role="alert" className="text-destructive text-xs">
          {error}
        </p>
      )}
    </form>
  );
}
