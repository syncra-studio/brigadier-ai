import { useRef, useState } from "react";
import { Button, ErrorLine } from "@/components/controls";
import { Spinner } from "@/components/icons";
import { failures } from "./dialogs";
import type { PluginsAdapter } from "./adapter";
import { signedIn, signingIn, signInLine } from "./model";
import type { PluginItemView } from "./types";

/** Shared inline actions on a server's card and detail page. */
export function SignInActions({ item, adapter, disabled = false, again = false }: {
  item: PluginItemView;
  adapter: PluginsAdapter | null;
  disabled?: boolean;
  again?: boolean;
}) {
  const [action, setAction] = useState<"start" | "stop" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const generation = useRef(0);
  const state = item.signIn;
  if (!state || state.type === "notRequired" || state.type === "cliSetupRequired") return null;
  const waiting = action === "start" || signingIn(state);
  if (signedIn(state) && !again && !waiting) return null;
  const run = (kind: "start" | "stop") => {
    const current = ++generation.current;
    setAction(kind);
    setError(null);
    const operation = kind === "start" ? adapter?.signIn?.(item.key, item.key, null) : adapter?.cancelSignIn?.(item.key);
    void operation?.then((results) => {
      const failure = failures(results);
      if (failure) throw new Error(failure);
    }).catch((cause: unknown) => {
      if (generation.current === current) setError(cause instanceof Error ? cause.message : String(cause));
    }).finally(() => { if (generation.current === current) setAction(null); });
  };
  return (
    <div data-slot="sign-in-actions" className="flex flex-wrap items-center gap-2">
      {waiting ? <>
        <Button variant="ghost" size="sm" disabled><Spinner aria-hidden className="size-icon-sm animate-spin" />Waiting for browser…</Button>
        {adapter?.cancelSignIn && <Button variant="ghost" size="sm" disabled={action === "stop"} onClick={() => run("stop")}>Cancel</Button>}
      </> : adapter?.signIn && <Button variant="ghost" size="sm" disabled={disabled || action === "stop"} onClick={() => run("start")}>{signedIn(state) ? "Sign in again" : "Sign in"}</Button>}
      <ErrorLine error={error} />
    </div>
  );
}

export function SignInStatus({ item, adapter, disabled }: { item: PluginItemView; adapter: PluginsAdapter; disabled: boolean }) {
  if (!item.signIn || item.signIn.type === "notRequired") return null;
  return (
    <div data-slot="sign-in-status" className="flex flex-wrap items-center gap-x-3 gap-y-1">
      <p role="status" className="text-foreground/65 text-label">{signInLine(item.signIn)}</p>
      <SignInActions item={item} adapter={adapter} disabled={disabled} again />
    </div>
  );
}
