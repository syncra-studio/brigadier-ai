import type { ModelGroup } from "@/components/shared/model-selector";
import type {
  Heat,
  LimitHit,
  ModelChoice,
  OverrideTarget,
  ProviderKind,
  ProviderOverview,
  QualityTier,
  QuotaWindow,
  Route,
  TaskCategory,
} from "@/ipc/generated";
import { formatTime } from "@/lib/format";
import { modelName } from "@/lib/setup";

export { isExpensive } from "@/lib/setup";

/**
 * Models and limits in words: the names the Usage page, worker cards and the Providers page
 * share, and the sentences they build (limits, reset times).
 */

/** Vendors as sentences name them ("Claude opus hit its 5-hour limit"). */
export const VENDOR_LABELS: Record<ProviderKind, string> = {
  claude: "Claude",
  codex: "Codex",
};

export { PROVIDERS } from "@/lib/providerKinds";

export const CATEGORIES: readonly TaskCategory[] = [
  "scout",
  "research",
  "implement",
  "review",
  "merge",
  "verify",
  "operate",
  "design",
  "chat",
  "orchestrate",
];

/** A kind of work in a few words, as Settings › Routing's Model strength names it. */
export const PLAIN_KIND_NAMES: Record<TaskCategory, string> = {
  scout: "Explore the code",
  research: "Research",
  implement: "Write code",
  review: "Review changes",
  merge: "Resolve conflicts",
  verify: "Run checks",
  operate: "Use apps",
  design: "Make designs and images",
  chat: "Take over a chat",
  orchestrate: "Coordinate work",
  gitText: "Write commit messages",
};

/** A kind of work as "best for …" reads it: "Best for writing code and reviewing changes". */
export const PLAIN_KIND_WORDS: Record<TaskCategory, string> = {
  scout: "exploring code",
  research: "research",
  implement: "writing code",
  review: "reviewing changes",
  merge: "resolving conflicts",
  verify: "running checks",
  operate: "using apps",
  design: "design work",
  chat: "chat",
  orchestrate: "coordinating work",
  gitText: "writing commit messages",
};

/** A ranked place as the page names it: "Claude Opus 5.5", "Newest Claude opus", "Any Codex model". */
export function placeName(target: OverrideTarget, groups: readonly ModelGroup[]): string {
  switch (target.type) {
    case "vendor":
      return `Any ${VENDOR_LABELS[target.provider]} model`;
    case "family":
      return `Newest ${VENDOR_LABELS[target.provider]} ${target.family}`;
    case "model":
      return choiceName(groups, { provider: target.provider, model: target.id, effort: null });
  }
}

export const TIER_LABELS: Record<QualityTier, string> = {
  unrated: "Unrated",
  light: "Light",
  standard: "Standard",
  strong: "Strong",
  frontier: "Frontier",
};

/** Text and fill tones for a heat: status colors, always beside a label. */
export const HEAT_TONES: Record<Heat, { text: string; fill: string; stroke: string }> = {
  cool: { text: "text-muted-foreground", fill: "bg-muted-foreground/60", stroke: "stroke-muted-foreground" },
  warm: { text: "text-foreground", fill: "bg-foreground/70", stroke: "stroke-foreground" },
  hot: { text: "text-warning", fill: "bg-warning", stroke: "stroke-warning" },
  limited: { text: "text-destructive", fill: "bg-destructive", stroke: "stroke-destructive" },
};

/** "Claude Opus 5.5": a model with its vendor, from the live lists (the raw id when unknown). */
export function choiceName(groups: readonly ModelGroup[], choice: ModelChoice): string {
  return `${VENDOR_LABELS[choice.provider]} ${modelName(groups, choice)}`;
}

/** A window's usual names, for ids the quota snapshot in hand doesn't list. */
const WINDOW_NAMES: Record<string, string> = {
  five_hour: "5-hour",
  seven_day: "weekly",
};

/**
 * What a window is called in a sentence ("5-hour", "weekly"): its label in the provider's last
 * snapshot, else a known id's name, else the id.
 */
export function windowName(
  providers: readonly ProviderOverview[] | undefined,
  provider: ProviderKind,
  windowId: string,
): string {
  const overview = providers?.find((entry) => entry.provider === provider);
  const window =
    overview?.usage?.windows.find((state) => state.window.id === windowId)?.window ??
    overview?.quota?.windows.find((entry) => entry.id === windowId);
  const label = window?.label ?? WINDOW_NAMES[windowId] ?? windowId.replaceAll("_", " ");
  return label === "Weekly" ? "weekly" : label;
}

/** What a window is scoped to, if anything ("Opus only", "gpt-reserve bucket"). */
export function windowScope(window: QuotaWindow): string | null {
  const parts = [
    window.model ? `${window.model} only` : null,
    window.bucket ? `${window.bucket} bucket` : null,
  ].filter((part) => part !== null);
  return parts.length > 0 ? parts.join(" · ") : null;
}

/** Shortest window first; windows of unknown length last. */
export function byLength(a: QuotaWindow, b: QuotaWindow): number {
  return (
    (a.windowMinutes ?? Number.MAX_SAFE_INTEGER) - (b.windowMinutes ?? Number.MAX_SAFE_INTEGER)
  );
}

const weekdayTime = new Intl.DateTimeFormat(undefined, {
  weekday: "short",
  hour: "numeric",
  minute: "2-digit",
});
const dateTime = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" });

/** When something lifts: "21:10" today, "Thu 09:00" within the week, a date beyond. */
export function formatResetAt(ms: number, nowMs: number): string {
  const day = new Date(ms).setHours(0, 0, 0, 0);
  const today = new Date(nowMs).setHours(0, 0, 0, 0);
  const days = Math.round((day - today) / 86_400_000);
  if (days === 0) return formatTime(ms);
  if (days > 0 && days < 7) return weekdayTime.format(ms);
  return dateTime.format(ms);
}

/** The countdown routing writes into a limit's reason when it decides ("(resets in 2h 10m)"). */
const RESETS_IN = / \(resets in [^)]*\)/;

/**
 * A stored routing reason, read later: its countdown was true only when routing decided, so it
 * names the reset time instead ("(resets 21:10)"), or drops it when the reset isn't known.
 */
export function withResetTime(text: string, resetsAtMs: number | null, nowMs: number): string {
  return text.replace(RESETS_IN, resetsAtMs !== null ? ` (resets ${formatResetAt(resetsAtMs, nowMs)})` : "");
}

/** Whether a stored reason carries a countdown, so its view needs a clock. */
export function hasResetCountdown(text: string): boolean {
  return RESETS_IN.test(text);
}

/**
 * A route's reason with its reset as a time: the reset of the ranked place it names as passed
 * over (the first one; the reason names no other), else `waitReset`, the reset a task waiting
 * from the start waits for.
 */
export function routeReason(route: Route, nowMs: number, waitReset: number | null = null): string {
  const skipped = route.explanation?.ranking?.skipped[0];
  return withResetTime(route.reason, skipped?.resetsAtMs ?? waitReset, nowMs);
}

/** "hit its 5-hour limit (resets 21:10)", "hit its spend limit", "ran out of credits". */
export function limitPhrase(
  limit: LimitHit,
  provider: ProviderKind,
  providers: readonly ProviderOverview[] | undefined,
  nowMs: number,
): string {
  const resets = limit.resetsAtMs !== null ? ` (resets ${formatResetAt(limit.resetsAtMs, nowMs)})` : "";
  switch (limit.kind) {
    case "usageWindow":
      return limit.window
        ? `hit its ${windowName(providers, provider, limit.window)} limit${resets}`
        : `hit a usage limit${resets}`;
    case "spendControl":
      return `hit its spend limit${resets}`;
    case "credits":
      return `ran out of credits${resets}`;
  }
}

// ----- rules -----------------------------------------------------------------------------

/** "a, b and c". */
export function joinWords(words: readonly string[], last = "and"): string {
  if (words.length <= 1) return words.join("");
  return `${words.slice(0, -1).join(", ")} ${last} ${words.at(-1)}`;
}

/** "+1.5", "−2", "0": a score contribution with its sign. */
export function formatDelta(delta: number): string {
  const rounded = Math.round(delta * 10) / 10;
  if (rounded === 0) return "0";
  return rounded > 0 ? `+${rounded}` : `−${Math.abs(rounded)}`;
}

/** A score, to one decimal. */
export function formatScore(score: number): string {
  return (Math.round(score * 10) / 10).toString();
}
