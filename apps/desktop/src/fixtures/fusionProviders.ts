import type { ProviderKind, ProviderOverview } from "@/ipc/generated";

export const fusionProviders: ProviderOverview[] = (["claude", "codex"] as ProviderKind[]).map((provider) => ({
  provider, status: { provider, path: `/fixture/${provider}`, version: "fixture", loggedIn: true, authMethod: null, plan: null, guidance: null, compacts: false },
  models: null, quota: null, usage: null, error: null, checkedAtMs: Date.now(),
}));
