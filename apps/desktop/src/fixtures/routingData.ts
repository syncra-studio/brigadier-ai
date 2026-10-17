/**
 * A faked daemon's agents for the Providers and Routing fixtures: two agents with accounts and
 * models, the merged model list with ratings and what was learned, the user's rules and orders,
 * and what routing would pick for each kind of work.
 */
import { account, model, overview, snapshot, status, window } from "@/fixtures/agents";
import type {
  AccountView,
  Area,
  Factor,
  MergedModel,
  OverrideRule,
  Project,
  ProviderKind,
  ProviderOverview,
  QualityTier,
  Ranking,
  RouteCandidate,
  RoutePreview,
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

export const PROJECT = "proj-desktop";

export const projects: Record<string, Project> = {
  [PROJECT]: { id: PROJECT, name: "Brigadier Desktop", createdAtMs: 0, repos: [], prefs: {}, trust: [] } as unknown as Project,
};

const workerKinds: TaskCategory[] = ["scout", "research", "implement", "review", "merge", "verify", "operate"];

export const rules: OverrideRule[] = [
  // What Routing's old switch wrote for a model kept from tasks.
  { id: "old-switch", effect: "never", target: { type: "model", provider: "claude", id: "sonnet" }, categories: workerKinds, areas: [], projectId: null, createdAtMs: 1 },
  // A model seen after its agent's first list.
  { id: "new-model-0192", effect: "never", target: { type: "model", provider: "codex", id: "gpt-6-luna" }, categories: workerKinds, areas: [], projectId: null, createdAtMs: 2 },
  // A rule of the user's own.
  { id: "never-sonnet-review", effect: "never", target: { type: "model", provider: "claude", id: "sonnet" }, categories: ["review"], areas: [], projectId: null, createdAtMs: 3 },
];

const place = (provider: ProviderKind, id: string, effort: string | null = null) => ({
  target: { type: "model" as const, provider, id },
  effort,
});

export const rankings: Ranking[] = [
  { id: "rank-implement", category: "implement", areas: [], projectId: null, manual: true, entries: [place("claude", "opus"), place("codex", "gpt-6")], only: false, updatedAtMs: 1 },
  { id: "rank-review", category: "review", areas: [], projectId: null, manual: true, entries: [place("claude", "opus", "high"), place("codex", "gpt-6", "high")], only: false, updatedAtMs: 1 },
  { id: "rank-operate", category: "operate", areas: [], projectId: null, manual: true, entries: [place("codex", "gpt-6"), place("claude", "opus")], only: false, updatedAtMs: 1 },
  { id: "rank-operate-frontend", category: "operate", areas: ["frontend"], projectId: null, manual: true, entries: [place("codex", "gpt-6"), place("claude", "opus")], only: true, updatedAtMs: 1 },
];

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

export const mergedModels: MergedModel[] = [
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

export const usageView = (projectId: string | null): UsageView => ({
  providers: [],
  activity: [],
  models: mergedModels,
  learned: [
    { provider: "codex", model: "gpt-6", category: "review", samples: 3, successRate: 1, reviewPassRate: 0.86, avgRework: 1, verificationPassRate: null, medianDurationMs: null, medianTokens: null, adjustment: 0.4 },
    { provider: "codex", model: "gpt-6", category: "implement", samples: 6, successRate: 0.83, reviewPassRate: null, avgRework: null, verificationPassRate: null, medianDurationMs: null, medianTokens: null, adjustment: 0.2 },
  ],
  projectId,
  registry: { revision: 4, updated: "2026-10-13", source: "downloaded", models: 24, fetchedAtMs: Date.now() - 3 * 3_600_000, checkedAtMs: Date.now() - 3 * 3_600_000, error: null },
});

const factor = (label: string, delta: number): Factor => ({ label, delta });

function candidate(provider: ProviderKind, id: string, fields: Partial<RouteCandidate>): RouteCandidate {
  return {
    provider,
    model: id,
    tier: "frontier",
    score: null,
    factors: [],
    effort: null,
    heat: "cool",
    listed: null,
    chosen: false,
    trial: false,
    new: false,
    blocked: null,
    resetsAtMs: null,
    ...fields,
  };
}

/** What routing would pick for each kind: review by GPT-6 when Claude wrote the change. */
export function previewRoutes(areas: Area[], author: ProviderKind | null): RoutePreview[] {
  return KINDS.map((category) => {
    const codexFirst = category === "review" ? author !== "codex" : category === "operate" || category === "scout";
    const first: [ProviderKind, string] = codexFirst ? ["codex", "gpt-6"] : ["claude", "opus"];
    const second: [ProviderKind, string] = codexFirst ? ["claude", "opus"] : ["codex", "gpt-6"];
    const candidates: RouteCandidate[] = [
      candidate(first[0], first[1], {
        chosen: true,
        score: 9.15,
        effort: "high",
        factors: [
          factor("registry strength for review", 9),
          factor("learned here: 86% of its reviewed work passed first time over 3 tasks", 0.4),
          factor("Codex already runs 1 worker", -0.25),
        ],
      }),
      candidate(second[0], second[1], {
        score: 5.9,
        effort: "high",
        heat: "hot",
        factors: [
          factor("registry strength for review", 9.1),
          factor("learned here: 5 of 6 tasks succeeded", 0.6),
          factor("Claude's weekly window", -3.8),
        ],
      }),
    ];
    if (category === "review") {
      candidates.push(
        candidate("codex", "gpt-6-luna", { tier: "strong", new: true, blocked: "On trial — trial models never get review work" }),
        candidate("claude", "sonnet", { tier: "strong", blocked: "Never rule · reviewing changes, everywhere" }),
        candidate("codex", "gpt-5.5", { tier: "strong", blocked: "Strong tier — below the Frontier floor" }),
        candidate("claude", "claude-opus-4-6", { tier: "standard", blocked: "Standard tier — below the Frontier floor" }),
        candidate("codex", "gpt-5", { tier: "standard", blocked: "Standard tier — below the Frontier floor" }),
      );
    }
    const ranking = rankings.find(
      (entry) => entry.category === category && entry.projectId === null && entry.areas.length === 0 && entry.manual,
    );
    return {
      category,
      areas,
      outcome: {
        type: "chosen",
        choice: { provider: first[0], model: first[1], effort: "high" },
        reason:
          category === "review" && author === "claude"
            ? "Top eligible score. Claude wrote this change, so Codex reviews it."
            : "Top eligible score.",
        explanation: null,
      },
      candidates,
      rankingId: ranking?.id ?? null,
      places: (ranking?.entries ?? []).map((entry, index) => ({
        position: index + 1,
        target: entry.target,
        model: entry.target.type === "model" ? entry.target.id : null,
        chosen: index === 0 && entry.target.provider === first[0],
        why: null,
        resetsAtMs: null,
      })),
      trialSlot: false,
    } satisfies RoutePreview;
  });
}
