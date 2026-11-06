import { rowModel, shortPlaceName, workName, type WorkRow } from "@/app/settings/routing/words";
import { ROLE_KINDS } from "@/app/settings/routing/roles";
import { effortLabel, findModel, type ModelGroup } from "@/components/shared/model-selector";
import type { GuidanceRow, ModelChoice, ModelInfo, ModelRule, ProviderKind } from "@/ipc/generated";
import { joinWords } from "@/lib/routing";
import { cliFacts } from "@/lib/providerKinds";
import { type Availability, modelAvailable, providerOn } from "@/state/providers";

/*
 * The Rules card's words and sums: a rule as one plain sentence, the effort a choice runs at
 * under the effort rules (a cap or a minimum moves it), which rule refuses a row, and the one
 * line under a rule that stops a kind of work from starting. The rules are those of the user's
 * rules file, `~/.brigadier/models.md`; a rule holds everywhere or for one kind of work (a row's
 * Work), and an effort rule may name an agent or one model.
 */

/** The efforts a ceiling may name, lowest first, as the rules file ranks them. */
export const EFFORT_LEVELS = ["none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra"] as const;

/** An effort's place among the levels; -1 for one the file doesn't know. */
export const effortRank = (effort: string) => EFFORT_LEVELS.indexOf(effort.toLowerCase() as (typeof EFFORT_LEVELS)[number]);

/** The names the words need: a rule's model pattern. */
export type Names = {
  /** A pattern of a rule in a few words, for an agent or any: "fable" reads "Fable", a family; "claude-opus-5-5" reads "Opus 5.5"; "*" reads "any model". */
  pattern: (agent: ProviderKind | null, pattern: string) => string;
};

/** The names the rules' words need, from the model list. */
export function ruleNames(groups: readonly ModelGroup[]): Names {
  const pattern = (agent: ProviderKind | null, name: string) => {
    if (name === "*") return "any model";
    if (name === "default") return agent ? shortPlaceName({ type: "vendor", provider: agent }, groups) : "the default model";
    // A family word covers every model of the family, today's and newer ones: never one version's name.
    if (/^\p{L}+$/u.test(name)) return name.charAt(0).toUpperCase() + name.slice(1);
    const listed = (agent ? groups.filter((group) => group.provider === agent) : groups)
      .map((group) => findModel(groups, { provider: group.provider, model: name, effort: null }))
      .find(Boolean);
    return listed?.displayName ?? name.charAt(0).toUpperCase() + name.slice(1);
  };
  return { pattern };
}

/** A model the agents list now, as a rule's preview checks it. */
export type ListedModel = {
  provider: ProviderKind;
  id: string;
  resolved: string | null;
  name: string;
  /** The agent's default model, which a row of `default` starts on. */
  isDefault?: boolean;
  /** The effort it starts at when none is named. */
  defaultEffort?: string | null;
  /** An older model: a family word never starts on it. */
  legacy?: boolean;
};

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

/** An effort rule's agent or model, as the file writes it after the directive. */
const about = (rule: { agent?: ProviderKind; model?: string }) => (rule.agent ? ` ${rule.agent}` : rule.model ? ` ${rule.model}` : "");

/** A rule as the file writes it: `forbid: fable`, `allow claude: opus, sonnet`, `effort-ceiling opus for Review: medium`, `advice: …`. */
export function ruleText(rule: ModelRule): string {
  if (rule.type === "advice") return `advice: ${rule.text}`;
  const scope = rule.work ? ` for ${rule.work}` : "";
  switch (rule.type) {
    case "forbid":
      return `forbid${scope}: ${rule.names.join(", ")}`;
    case "allow":
      return `allow${rule.agent ? ` ${rule.agent}` : ""}${scope}: ${rule.names.join(", ")}`;
    case "effortCeiling":
      return `effort-ceiling${about(rule)}${scope}: ${rule.effort}`;
    case "effortFloor":
      return `effort-floor${about(rule)}${scope}: ${rule.effort}`;
  }
}

/** What an effort rule is about, in a word or two ("Opus", "Codex"); `null`, every model. */
const effortTarget = (rule: { agent?: ProviderKind; model?: string }, names: Names): string | null =>
  rule.agent ? cliFacts(rule.agent).vendor : rule.model ? names.pattern(null, rule.model) : null;

/**
 * A rule as one plain sentence: "Never use Fable", "Never use Sonnet for Adversarial review",
 * "Only use Opus or Sonnet on Claude", "Never go above High effort", "Never run Opus above
 * Medium", "Adversarial review never below High"; advice as its own words.
 */
export function ruleTitle(rule: ModelRule, names: Names): string {
  if (rule.type === "advice") return rule.text;
  const scope = rule.work ? ` for ${workName(rule.work)}` : "";
  switch (rule.type) {
    case "forbid": {
      const models = joinWords(rule.names.map((name) => names.pattern(null, name)), "or");
      return `Never use ${models}${scope}`;
    }
    case "allow": {
      const models = joinWords(rule.names.map((name) => names.pattern(rule.agent, name)), "or");
      return `Only use ${models}${rule.agent ? ` on ${cliFacts(rule.agent).vendor}` : ""}${scope}`;
    }
    case "effortCeiling":
    case "effortFloor": {
      const way = rule.type === "effortCeiling" ? "above" : "below";
      const effort = effortLabel(rule.effort);
      const target = effortTarget(rule, names);
      if (target) return `Never run ${target} ${way} ${effort}${scope}`;
      if (rule.work) return `${workName(rule.work)} never ${way} ${effort}`;
      return `Never go ${way} ${effort} effort`;
    }
  }
}

/** Whether a rule holds for a start of `work` (`null`: a chat, a session, or a start for no work). */
const holdsFor = (rule: ModelRule, work: string | null) =>
  rule.type !== "advice" && (!rule.work || (work !== null && rule.work.trim().toLowerCase() === work.trim().toLowerCase()));

/** A choice a rule may refuse: its agent, the names its model goes by, its effort. */
export type Choice = { provider: ProviderKind; names: readonly string[]; effort: string | null };

type EffortRule = Extract<ModelRule, { type: "effortCeiling" | "effortFloor" }>;

/** The effort rules that hold for a choice: the lowest cap and the highest minimum, as the daemon sums them. */
export function effortLimits(rules: readonly ModelRule[], choice: Choice, work: string | null): { ceiling: EffortRule | null; floor: EffortRule | null } {
  let ceiling: EffortRule | null = null;
  let floor: EffortRule | null = null;
  for (const rule of rules) {
    if ((rule.type !== "effortCeiling" && rule.type !== "effortFloor") || !holdsFor(rule, work)) continue;
    if (rule.agent && rule.agent !== choice.provider) continue;
    if (rule.model && !named([rule.model], choice.names)) continue;
    if (rule.type === "effortCeiling" && (!ceiling || effortRank(rule.effort) < effortRank(ceiling.effort))) ceiling = rule;
    if (rule.type === "effortFloor" && (!floor || effortRank(rule.effort) > effortRank(floor.effort))) floor = rule;
  }
  return { ceiling, floor };
}

/**
 * The first rule that refuses `choice` for a start of `work`, as the daemon judges it; `null`
 * if none does. A cap or minimum alone moves the effort and refuses nothing; a cap and a
 * minimum that can't both be met refuse it (the cap is the rule named).
 */
export function refusingRule(rules: readonly ModelRule[], choice: Choice, work: string | null): ModelRule | null {
  const model = rules.find((rule) => {
    if (!holdsFor(rule, work)) return false;
    switch (rule.type) {
      case "forbid":
        return named(rule.names, choice.names);
      case "allow":
        return (rule.agent === null || rule.agent === choice.provider) && choice.names.length > 0 && !named(rule.names, choice.names);
      default:
        return false;
    }
  });
  if (model) return model;
  const { ceiling, floor } = effortLimits(rules, choice, work);
  return ceiling && floor && effortRank(floor.effort) > effortRank(ceiling.effort) ? ceiling : null;
}

/** The effort `choice` runs at under the effort rules: moved down to a cap or up to a minimum; as asked within them. */
export function ruledEffort(rules: readonly ModelRule[], choice: Choice, work: string | null): string | null {
  const { ceiling, floor } = effortLimits(rules, choice, work);
  const asked = choice.effort === null ? -1 : effortRank(choice.effort);
  if (ceiling && choice.effort !== null && (asked < 0 || asked > effortRank(ceiling.effort))) return ceiling.effort;
  if (floor && asked < effortRank(floor.effort)) return floor.effort;
  return choice.effort;
}

/**
 * A row as a choice, as the daemon judges its start: the names its model goes by (as written, and
 * as the listed model it is; for the CLI's default, the model the agent lists as its default) and
 * its effort (the model's default when the row names none).
 */
export function rowChoice(row: GuidanceRow, models: readonly ListedModel[]): Choice {
  const fallback = row.model === "default";
  const listed = models.find(
    (model) => model.provider === row.agent && (fallback ? model.isDefault : model.id === row.model || model.resolved === row.model),
  );
  return {
    provider: row.agent,
    names: [...(fallback ? [] : [row.model]), ...(listed ? [listed.id, ...(listed.resolved ? [listed.resolved] : [])] : [])],
    effort: row.effort ?? listed?.defaultEffort ?? null,
  };
}

/**
 * The listed model a row starts on, as the daemon's `listed_model` picks it: the agent's default
 * for `default`, a listed id or alias, else a family word's newest model; null when none is listed.
 */
export function rowListed(row: GuidanceRow, models: readonly ListedModel[]): ListedModel | null {
  const own = models.filter((model) => model.provider === row.agent);
  return row.model === "default" ? (own.find((model) => model.isDefault) ?? null) : rowModel(own, row.model);
}

/** Whether a row can start as Settings › Providers stands: its agent is on and the model it starts on isn't made unavailable. */
export function rowAvailable(row: GuidanceRow, models: readonly ListedModel[], availability: Availability): boolean {
  const listed = rowListed(row, models);
  return providerOn(availability, row.agent) && (!listed || modelAvailable(availability, { provider: row.agent, id: listed.id }));
}

/**
 * The row a kind of work starts on now, as a start by its Work picks it: the first that your rules
 * let start and Settings › Providers lets run; `undefined` when none can.
 */
export function startsOn(
  rows: readonly GuidanceRow[],
  rules: readonly ModelRule[],
  models: readonly ListedModel[],
  availability: Availability,
  work: string,
): GuidanceRow | undefined {
  return rows.find((row) => !refusingRule(rules, rowChoice(row, models), work) && rowAvailable(row, models, availability));
}

/**
 * A chat's or session's pick as a choice: the names its model goes by (as picked, its id, what
 * it resolves to; the agent's default model when none is picked) and its effort (the model's
 * default when none is picked), as the daemon judges a start.
 */
export function pickedChoice(groups: readonly ModelGroup[], choice: ModelChoice): Choice {
  const model = findModel(groups, choice);
  const names = [choice.model, model?.id, model?.resolved].filter((name): name is string => !!name && name !== "default");
  return { provider: choice.provider, names: [...new Set(names)], effort: choice.effort ?? model?.defaultEffort ?? null };
}

/** The pick of `model` at `effort` (`null`: the model alone, its effort not judged). */
const modelPick = (provider: ProviderKind, model: ModelInfo, effort: string | null): Choice => ({
  provider,
  names: [model.id, ...(model.resolved ? [model.resolved] : [])],
  effort,
});

/**
 * Why the rules keep a model out of a chat or session, or one of its efforts (`effort` null:
 * the model itself), in the Rules card's words ("Never go above High effort"); `null` if
 * they don't. An effort above a cap or below a minimum is kept out of the menus.
 */
export function pickRefusal(
  rules: readonly ModelRule[],
  names: Names,
  provider: ProviderKind,
  model: ModelInfo,
  effort: string | null,
  /** The kind of work the pick is for, for its own rules too; none: a chat or session. */
  work: string | null = null,
): string | null {
  const pick = modelPick(provider, model, effort);
  const rule = refusingRule(rules, pick, work);
  if (rule) return ruleTitle(rule, names);
  if (effort === null) return null;
  const { ceiling, floor } = effortLimits(rules, pick, work);
  const rank = effortRank(effort);
  const outside = ceiling && (rank < 0 || rank > effortRank(ceiling.effort)) ? ceiling : floor && rank < effortRank(floor.effort) ? floor : null;
  return outside ? ruleTitle(outside, names) : null;
}

/**
 * A pick the rules allow, as near the one asked for as can be: an effort above a cap comes down
 * to the highest of its model's efforts within the rules, one below a minimum goes up to the
 * lowest. Any other pick stays as it is.
 */
export function allowedPick(rules: readonly ModelRule[], groups: readonly ModelGroup[], choice: ModelChoice): ModelChoice {
  const model = findModel(groups, choice);
  if (!model || rules.length === 0) return choice;
  const picked = pickedChoice(groups, choice);
  if (picked.effort === null || refusingRule(rules, picked, null)) return choice;
  const ruled = ruledEffort(rules, picked, null);
  if (ruled === picked.effort) return choice;
  const within = (effort: string) => ruledEffort(rules, { ...picked, effort }, null) === effort;
  const up = effortRank(ruled ?? "") > effortRank(picked.effort);
  const listed = up ? model.efforts.find(within) : model.efforts.toReversed().find(within);
  return listed ? { ...choice, effort: listed } : choice;
}

/** A rules-file line as the daemon quotes it in a refusal, compared without case or spaces. */
const lineKey = (line: string) => line.toLowerCase().replace(/\s+/g, "");

/**
 * An error in the Rules card's words when it is the daemon's refusal by the user's rules
 * (`refused by your model rule: "effort-ceiling: high"`): "Refused by your rule “Never go above
 * High effort”"; two that can't both be met, both named. Any other error stays as it is.
 */
export function refusalText(message: string, rules: readonly ModelRule[], names: Names): string {
  const title = (quoted: string) => {
    const rule = rules.find((found) => lineKey(ruleText(found)) === lineKey(quoted));
    return rule ? ruleTitle(rule, names) : null;
  };
  const both = /refused by your model rules: "(.*?)" and "(.*?)" can't both be met/s.exec(message);
  if (both) {
    const [first, second] = [title(both[1]!), title(both[2]!)];
    return first && second ? `Refused: your rules “${first}” and “${second}” can't both be met` : message;
  }
  const quoted = /refused by your model rule: "(.*)"/s.exec(message)?.[1];
  if (quoted === undefined) return message;
  const one = title(quoted);
  return one ? `Refused by your rule “${one}”` : message;
}

/**
 * The one line under each rule that stops work, keyed by the rule's text. A kind of work with no
 * row that may run can't start: "Docs can't start: this rule keeps out Opus." A kind of roles
 * (review, Fallback) with a side that still may run can start, so the line names only the side
 * it stops: "Reviews Codex's work: this rule keeps out Opus 5.5."
 */
export function ruleWarnings(
  rules: readonly ModelRule[],
  kinds: readonly WorkRow[],
  models: readonly ListedModel[],
  names: Names,
  modelName: (row: GuidanceRow) => string,
): Map<string, string> {
  const warnings = new Map<string, string[]>();
  for (const kind of kinds) {
    const refused = kind.rows.map((row) => {
      const choice = rowChoice(row, models);
      return { row, choice, rule: refusingRule(rules, choice, kind.work) };
    });
    const roles = ROLE_KINDS[kind.work];
    const none = refused.every((found) => found.rule);
    const stopped = none ? refused.slice(0, 1) : roles ? refused.filter((found) => found.rule) : [];
    for (const { rule, choice, row } of stopped) {
      if (!rule) continue;
      const who = none ? `${workName(kind.work)} can't start` : roles!.role(row);
      let line: string;
      if (rule.type === "effortCeiling") {
        const { floor } = effortLimits(rules, choice, kind.work);
        const target = effortTarget(rule, names) ?? modelName(row);
        line = `${who}: it needs ${effortLabel(floor?.effort ?? rule.effort)}, and this rule keeps ${target} at ${effortLabel(rule.effort)}.`;
      } else {
        const out = none ? joinWords([...new Set(refused.filter((found) => found.rule === rule).map((found) => modelName(found.row)))]) : modelName(row);
        line = `${who}: this rule keeps out ${out}.`;
      }
      const key = ruleText(rule);
      warnings.set(key, [...(warnings.get(key) ?? []), line]);
    }
  }
  return new Map([...warnings].map(([key, lines]) => [key, lines.join(" ")]));
}
