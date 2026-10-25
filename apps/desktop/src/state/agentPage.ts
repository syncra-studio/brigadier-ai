import type { ProviderKind } from "@/ipc/generated";
import { select } from "@/state/actions";
import type { SettingsSelection } from "@/state/store";

import { useAgentPageAsk } from "@/state/agentSignIn";

export { useAgentPageAsk } from "@/state/agentSignIn";

/** Opens Settings › Providers at `provider`'s page, starting its sign-in when `signIn`. */
export function openAgentPage(provider: ProviderKind, signIn = false): void {
  const destination: SettingsSelection = { type: "settings", page: "providers", detail: { provider } };
  select(destination);
  if (signIn) useAgentPageAsk.setState({ ask: { provider, under: destination } });
}
