import { create } from "zustand";

import type { ProviderKind } from "@/ipc/generated";
import type { SettingsSelection } from "@/state/store";

/**
 * An agent's page in Settings › Providers asked for from elsewhere (the usage list's Sign in):
 * sign-in is a one-shot request attached to this visit, never part of Back/Forward history.
 */
export const useAgentPageAsk = create<{ ask: { provider: ProviderKind; under: SettingsSelection } | null }>(() => ({
  ask: null,
}));
