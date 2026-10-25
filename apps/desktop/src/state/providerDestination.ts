import type { ProviderKind } from "@/ipc/generated";
import { PROVIDERS } from "@/lib/providerKinds";
import { useApp } from "@/state/store";

/** Resolve a search/reveal before navigation, so rendering cannot reopen a page after Back. */
export function providerForSetting(label: string): ProviderKind | null {
  const { selection, providers } = useApp.getState();
  const overviews = providers.view?.providers;
  const modelsAgent = overviews?.find((entry) => entry.models?.models.some((model) => model.displayName === label));
  if (modelsAgent) return modelsAgent.provider;
  if (!["Installation", "Accounts", "Models"].includes(label)) return null;
  if (selection.type === "settings" && selection.page === "providers" && selection.detail) {
    return selection.detail.provider;
  }
  // As on the overview, an agent still being checked is not known to be missing.
  return PROVIDERS.find((provider) => {
    const status = overviews?.find((entry) => entry.provider === provider)?.status;
    return !status || !!status.path;
  }) ?? PROVIDERS[0];
}
