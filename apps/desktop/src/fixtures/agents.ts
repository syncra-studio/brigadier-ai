/** Agents, models and accounts for the Settings fixtures: synthetic, nothing real. */
import type {
  AccountView,
  ModelInfo,
  ProviderKind,
  ProviderOverview,
  ProviderStatus,
  QuotaSnapshot,
  QuotaWindow,
} from "@/ipc/generated";

export function model(id: string, displayName: string, fields: Partial<ModelInfo> = {}): ModelInfo {
  return {
    id,
    displayName,
    description: "",
    resolved: null,
    efforts: [],
    defaultEffort: null,
    isDefault: false,
    inputModalities: [],
    fast: null,
    legacy: false,
    ...fields,
  };
}

export function status(provider: ProviderKind, fields: Partial<ProviderStatus> = {}): ProviderStatus {
  return {
    provider,
    path: `/usr/local/bin/${provider}`,
    version: "2.1.0",
    loggedIn: true,
    authMethod: "subscription",
    plan: "max",
    guidance: null,
    compacts: true,
    ...fields,
  };
}

export function overview(provider: ProviderKind, models: ModelInfo[]): ProviderOverview {
  return {
    provider,
    status: status(provider),
    models: { provider, models, cliVersion: "2.1.0", fetchedAtMs: 0 },
    quota: null,
    usage: null,
    error: null,
    checkedAtMs: Date.now(),
  };
}

/** Claude's and Codex's models, Opus 5.5 and GPT-6 their defaults. */
export const OVERVIEWS: ProviderOverview[] = [
  overview("claude", [
    model("opus", "Opus 5.5", { isDefault: true, description: "The most capable, for complex work" }),
    model("sonnet", "Sonnet 5.5", { description: "Fast and capable, for everyday work" }),
  ]),
  overview("codex", [
    model("gpt-6", "GPT-6", { isDefault: true, description: "The default for coding work" }),
  ]),
];

export function window(id: string, label: string, usedPercent: number, fields: Partial<QuotaWindow> = {}): QuotaWindow {
  return { id, label, usedPercent, resetsAtMs: null, windowMinutes: null, bucket: null, model: null, ...fields };
}

export function snapshot(provider: ProviderKind, windows: QuotaWindow[], observedAtMs = Date.now()): QuotaSnapshot {
  return { provider, windows, limit: null, observedAtMs, source: "read" };
}

export function account(
  provider: ProviderKind,
  id: string | undefined,
  email: string,
  fields: Partial<AccountView> = {},
): AccountView {
  return {
    account: id === undefined ? { provider } : { provider, account: id },
    name: "",
    default: id === undefined,
    status: status(provider, { email }),
    quota: null,
    quotaAtMs: null,
    checkedAtMs: Date.now(),
    checking: false,
    checkedGeneration: 0,
    error: null,
    ...fields,
    // New work starts on the default unless the fixture says otherwise (switching picked another).
    inUse: fields.inUse ?? fields.default ?? id === undefined,
  };
}
