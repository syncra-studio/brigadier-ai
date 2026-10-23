/**
 * A faked daemon's agents for the Providers fixture: two agents with accounts and models, and
 * the merged model list with ratings.
 */
import { account, model, overview, snapshot, status, window } from "@/fixtures/agents";
import type {
  AccountView,
  MergedModel,
  Project,
  ProviderKind,
  ProviderOverview,
  QualityTier,
  TaskCategory,
  UsageView,
} from "@/ipc/generated";

const quota = (provider: ProviderKind, fiveHour: number, week: number) =>
  snapshot(provider, [
    window("five_hour", "5-hour", fiveHour, { windowMinutes: 300 }),
    window("seven_day", "Weekly", week, { windowMinutes: 10_080 }),
  ]);

const signedIn = (provider: ProviderKind, id: string | undefined, email: string, fields: Partial<AccountView> = {}) =>
  account(provider, id, email, { quota: quota(provider, 32, 69), quotaAtMs: Date.now(), ...fields });

const efforts = { efforts: ["low", "medium", "high"] };

export const accounts: AccountView[] = [
  signedIn("claude", undefined, "stephen.golban@example.com"),
  signedIn("claude", "acct-work", "work.person@corp.example.io", { name: "work.person@corp.example.io", quota: quota("claude", 84, 93) }),
  signedIn("claude", "acct-side", "side@example.org", { name: "Side project" }),
  signedIn("codex", undefined, "stephen@example.com"),
];

export const overviews: ProviderOverview[] = [
  overview("claude", [
    model("opus", "Opus 5.5", { isDefault: true, ...efforts }),
    model("sonnet", "Sonnet 5.5", efforts),
    model("claude-fable-5-1", "Fable 5.1", efforts),
    model("claude-opus-4-6", "Opus 4.6", { legacy: true, ...efforts }),
  ]),
  { ...overview("codex", [
    model("gpt-6", "GPT-6", { isDefault: true, ...efforts }),
    model("gpt-6-luna", "GPT-6 Luna", efforts),
    model("gpt-5.5", "GPT-5.5", efforts),
  ]), status: status("codex", { plan: "plus" }) },
];

const PROJECT = "proj-desktop";

export const projects: Record<string, Project> = {
  [PROJECT]: { id: PROJECT, name: "Brigadier Desktop", createdAtMs: 0, repos: [], prefs: {}, trust: [] } as unknown as Project,
};

const KINDS: TaskCategory[] = ["scout", "research", "implement", "review", "merge", "verify", "operate", "chat", "orchestrate", "gitText"];

function merged(
  provider: ProviderKind,
  id: string,
  displayName: string,
  tier: QualityTier,
  scores: number[],
  fields: Partial<MergedModel> = {},
): MergedModel {
  return {
    provider,
    id,
    resolved: null,
    displayName,
    status: "curated",
    ratingProvenance: "curated",
    defaultEffort: { scout: "medium", research: "medium", implement: "high", review: "high", merge: "high", verify: "medium", operate: "medium", chat: "medium", orchestrate: "medium", gitText: "low" },
    registryKey: id,
    family: null,
    tier,
    only: null,
    strengths: Object.fromEntries(KINDS.map((kind, index) => [kind, scores[index]])),
    areaStrengths: { frontend: -0.5, backend: 1, infra: 0.5, docs: -1, tests: 0.5 },
    efforts: ["low", "medium", "high"],
    contextWindow: 400_000,
    knowledgeCutoff: null,
    modalities: { input: ["text", "image"], output: ["text"], tools: [] },
    legacy: false,
    excluded: false,
    trial: null,
    research: null,
    ...fields,
  };
}

const mergedModels: MergedModel[] = [
  merged("claude", "opus", "Opus 5.5", "frontier", [6.5, 8.6, 9.2, 9.1, 9.0, 6.0, 8.4, 8.0, 9.0, 5.0], { contextWindow: 1_000_000, family: "opus" }),
  merged("claude", "sonnet", "Sonnet 5.5", "strong", [8.2, 8.3, 8.4, 8.0, 7.8, 7.6, 8.1, 8.6, 8.2, 6.8], { contextWindow: 1_000_000, family: "sonnet" }),
  merged("claude", "claude-opus-4-6", "Opus 4.6", "standard", [4.8, 6.8, 7.2, 7.2, 7.0, 4.6, 6.0, 6.2, 7.0, 4.0], { legacy: true, contextWindow: 200_000 }),
  merged("claude", "claude-fable-5-1", "Fable 5.1", "frontier", [9, 9, 9, 9, 9, 9, 9, 9, 9, 9], { excluded: true }),
  merged("codex", "gpt-6", "GPT-6", "frontier", [6.8, 8.9, 9.0, 9.0, 8.8, 6.4, 8.0, 7.6, 8.7, 5.2]),
  merged("codex", "gpt-5.5", "GPT-5.5", "strong", [7.8, 8.2, 8.0, 7.9, 7.6, 7.5, 7.1, 7.5, 7.7, 6.8]),
  merged("codex", "gpt-6-luna", "GPT-6 Luna", "strong", [7.8, 8.3, 8.1, 8.0, 7.7, 7.6, 7.2, 7.6, 7.8, 6.8], {
    status: "inherited",
    trial: { outcomes: 1, needed: 3 },
  }),
];

export const usageView = (): UsageView => ({
  providers: [],
  activity: [],
  models: mergedModels,
  registry: { revision: 4, updated: "2026-10-13", source: "downloaded", models: 24, fetchedAtMs: Date.now() - 3 * 3_600_000, checkedAtMs: Date.now() - 3 * 3_600_000, error: null },
});
