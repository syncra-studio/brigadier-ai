import { create } from "zustand";

import type { ProviderKind } from "@/ipc/generated";
import { openSettings } from "@/state/actions";

/**
 * An agent's page in Settings › Providers asked for from elsewhere (the usage list's Sign in):
 * the Providers page opens it, with its sign-in started when `signIn`, and clears the ask.
 */
export const useAgentPageAsk = create<{ ask: { provider: ProviderKind; signIn: boolean } | null }>(() => ({
  ask: null,
}));

/** Opens Settings › Providers at `provider`'s page, starting its sign-in when `signIn`. */
export function openAgentPage(provider: ProviderKind, signIn = false): void {
  useAgentPageAsk.setState({ ask: { provider, signIn } });
  openSettings("providers");
}
