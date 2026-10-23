import type { WorkRow } from "@/app/settings/routing/words";
import { effortLabel } from "@/components/shared/model-selector";
import type { ModelRule, OverrideTarget, ProviderKind } from "@/ipc/generated";
import { joinWords, VENDOR_LABELS } from "@/lib/routing";

/*
 * The Rules card's words and sums: a rule's title and line, the rules a draft makes, and what a
 * draft would change before it is added (or why it would change nothing). The rules are those of
 * the user's rules file, `~/.brigadier/models.md`.
 */

/** What a new rule does: keeps a model out, allows only some models, or caps the effort. */
export type DraftEffect = "never" | "only" | "cap";

/** A rule being written: what it does, to which model (never, only) or up to which effort (cap). */
export type Draft = { effect: DraftEffect; target: OverrideTarget | null; effort: string };

/** The efforts a ceiling may name, lowest first, as the rules file ranks them. */
export const EFFORT_LEVELS = ["none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra"] as const;

/** An effort's place among the levels; -1 for one the file doesn't know. */
export const effortRank = (effort: string) => EFFORT_LEVELS.indexOf(effort.toLowerCase() as (typeof EFFORT_LEVELS)[number]);

/** The names the words need: a rule's model pattern, a menu's pick. */
export type Names = {
  /** A pattern of a rule ("fable", "claude-opus-5-5", "*") in a few words, for an agent or any. */
  pattern: (agent: ProviderKind | null, pattern: string) => string;
  target: (target: OverrideTarget) => string;
};

/** A model the agents list now, as a rule's preview checks it. */
export type ListedModel = { provider: ProviderKind; id: string; resolved: string | null; name: string };

/** Whether a rule's pattern names a model: `*`, the exact id, or one of its words (as the daemon matches). */
export function matches(pattern: string, model: string): boolean {
  const p = pattern.toLowerCase();
  return (
    p === "*" ||
    model.toLowerCase() === p ||
    model
      .split(/[^\p{L}\p{N}]/u)
      .some((word) => word.toLowerCase() === p)
  );
}

/** Whether any of a model's names (its id, what it resolves to) matches any pattern. */
const named = (patterns: readonly string[], names: readonly string[]) =>
  names.some((name) => patterns.some((pattern) => matches(pattern, name)));

/** A rule as the file writes it: `forbid: fable`, `allow claude: opus, sonnet`, `effort-ceiling: high`. */
export function ruleText(rule: ModelRule): string {
  switch (rule.type) {
    case "forbid":
      return `forbid: ${rule.names.join(", ")}`;
    case "allow":
      return `allow${rule.agent ? ` ${rule.agent}` : ""}: ${rule.names.join(", ")}`;
    case "effortCeiling":
      return `effort-ceiling: ${rule.effort}`;
  }
}

/** "Never use Fable", "Only use Opus or Sonnet on Claude", "Never go above High effort". */
export function ruleTitle(rule: ModelRule, names: Names): string {
  switch (rule.type) {
    case "forbid":
      return `Never use ${joinWords(rule.names.map((name) => names.pattern(null, name)), "or")}`;
    case "allow": {
      const models = joinWords(rule.names.map((name) => names.pattern(rule.agent, name)), "or");
      return rule.agent ? `Only use ${models} on ${VENDOR_LABELS[rule.agent]}` : `Only use ${models}`;
    }
    case "effortCeiling":
      return `Never go above ${effortLabel(rule.effort)} effort`;
  }
}

/** Where a rule applies and what it does: "Everywhere · Chats and workers can't use it". */
export function ruleLine(rule: ModelRule): string {
  switch (rule.type) {
    case "forbid":
      return `Everywhere · Chats and workers can't use ${rule.names.length === 1 ? "it" : "them"}`;
    case "allow":
      return `${rule.agent ? `${VENDOR_LABELS[rule.agent]} only` : "Everywhere"} · Chats and workers use nothing else`;
    case "effortCeiling":
      return "Everywhere · Chats and workers stay at or below it";
  }
}

/** The word of a draft's model the rule names: a listed id, a family word, or `default`. */
function draftName(target: OverrideTarget): string {
  return target.type === "model" ? target.id : target.type === "family" ? target.family : "default";
}

/**
 * The rules once a finished draft is added: Never adds its own line; Only joins the allowlist
 * of the model's agent when there is one (two allowlists would allow only what both name); a
 * ceiling replaces the one there.
 */
export function draftRules(draft: Draft, rules: readonly ModelRule[]): ModelRule[] {
  if (draft.effect === "cap") {
    const rest = rules.filter((rule) => rule.type !== "effortCeiling");
    const at = rules.findIndex((rule) => rule.type === "effortCeiling");
    const ceiling: ModelRule = { type: "effortCeiling", effort: draft.effort };
    return at < 0 ? [...rules, ceiling] : [...rest.slice(0, at), ceiling, ...rest.slice(at)];
  }
  const target = draft.target;
  if (!target) return [...rules];
  const name = draftName(target);
  if (draft.effect === "never") return [...rules, { type: "forbid", names: [name] }];
  const own = rules.findIndex((rule) => rule.type === "allow" && rule.agent === target.provider);
  if (own < 0) return [...rules, { type: "allow", agent: target.provider, names: [name] }];
  return rules.map((rule, index) =>
    index === own && rule.type === "allow" && !rule.names.includes(name) ? { ...rule, names: [...rule.names, name] } : rule,
  );
}

/** What a draft would change, said before it is added; a warning when it would change nothing or clash. */
export type DraftPreview = { tone: "info" | "warning"; text: string };

/** "“Bounded implementation” and “Review”": the kinds of work rows name. */
const works = (rows: readonly WorkRow[]) => joinWords(rows.map((row) => `“${row.work}”`));

/**
 * What adding the draft would do, from the models the agents list now and the rows in force; a
 * warning when it is already a rule, or a row would be refused by it.
 */
export function previewDraft(
  draft: Draft,
  {
    rules,
    rows,
    models,
    names,
  }: {
    rules: readonly ModelRule[];
    rows: readonly WorkRow[];
    models: readonly ListedModel[];
    names: Names;
  },
): DraftPreview | null {
  // The names a row's model goes by: as written, and as the listed model it is.
  const rowNames = (row: WorkRow) => {
    const listed = models.find((model) => model.provider === row.row.agent && (model.id === row.row.model || model.resolved === row.row.model));
    return [row.row.model, ...(listed ? [listed.id, ...(listed.resolved ? [listed.resolved] : [])] : [])];
  };
  if (draft.effect === "cap") {
    const ceiling = effortRank(draft.effort);
    const old = rules.find((rule) => rule.type === "effortCeiling");
    if (old?.type === "effortCeiling" && effortRank(old.effort) === ceiling) return { tone: "warning", text: "You already have this rule." };
    const above = rows.filter((row) => row.row.effort !== null && effortRank(row.row.effort) > ceiling);
    if (above.length > 0) {
      return {
        tone: "warning",
        text: `${works(above)} ${above.length === 1 ? "asks" : "ask"} for more than ${effortLabel(draft.effort)} effort, so starting ${above.length === 1 ? "it" : "them"} would be refused. Lower the effort there too.`,
      };
    }
    const replaces = old?.type === "effortCeiling" ? ` It replaces your ${effortLabel(old.effort)} ceiling.` : "";
    return { tone: "info", text: `Chats and workers will run at ${effortLabel(draft.effort)} effort or lower; asking for more is refused.${replaces}` };
  }
  const target = draft.target;
  if (!target) return null;
  const name = draftName(target);
  const model = names.target(target);
  if (draft.effect === "never") {
    if (rules.some((rule) => rule.type === "forbid" && rule.names.includes(name))) {
      return { tone: "warning", text: "You already have this rule." };
    }
    const hit = rows.filter((row) => named([name], rowNames(row)));
    if (hit.length > 0) {
      return {
        tone: "warning",
        text: `${works(hit)} ${hit.length === 1 ? "uses" : "use"} ${model}, so starting ${hit.length === 1 ? "it" : "them"} would be refused. Set your own model there too.`,
      };
    }
    const caught = models.filter((found) => named([name], [found.id, ...(found.resolved ? [found.resolved] : [])]));
    if (caught.length === 0) return { tone: "info", text: `No model listed now is ${model}; the rule applies once one is.` };
    return {
      tone: "info",
      text: `${joinWords(caught.map((found) => found.name))} won't be used: a chat or worker that asks for ${caught.length === 1 ? "it" : "them"} is refused, quoting this rule.`,
    };
  }
  const agent = target.provider;
  const vendor = VENDOR_LABELS[agent];
  const own = rules.find((rule) => rule.type === "allow" && rule.agent === agent);
  const before = own?.type === "allow" ? own.names : [];
  if (before.includes(name)) return { tone: "warning", text: "You already have this rule." };
  const allowed = [...before, name];
  const hit = rows.filter((row) => row.row.agent === agent && !named(allowed, rowNames(row)));
  if (hit.length > 0) {
    return {
      tone: "warning",
      text: `${works(hit)} ${hit.length === 1 ? "uses" : "use"} another ${vendor} model, so starting ${hit.length === 1 ? "it" : "them"} would be refused. Set your own model there too.`,
    };
  }
  const refused = models.filter(
    (found) => found.provider === agent && !named(allowed, [found.id, ...(found.resolved ? [found.resolved] : [])]),
  );
  const joins = own ? `${model} joins your Only rule for ${vendor}. ` : "";
  const rest =
    refused.length === 0
      ? `No ${vendor} model listed now is refused.`
      : `${joinWords(refused.map((found) => found.name))} ${refused.length === 1 ? "is" : "are"} refused.`;
  return { tone: "info", text: `${joins}${vendor} chats and workers will use only ${joinWords(allowed.map((found) => names.pattern(agent, found)), "or")}. ${rest}` };
}
