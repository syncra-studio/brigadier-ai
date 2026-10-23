import { shortPlaceName, type WorkRow } from "@/app/settings/routing/words";
import { effortLabel, findModel, type ModelGroup } from "@/components/shared/model-selector";
import type { GuidanceRow, ModelChoice, ModelInfo, ModelRule, OverrideTarget, ProviderKind } from "@/ipc/generated";
import { joinWords, VENDOR_LABELS } from "@/lib/routing";

/*
 * The Rules card's words and sums: a rule's title and line, the rules a draft makes, what a
 * draft would change before it is added (or why it would change nothing), and which rule
 * refuses a row. The rules are those of the user's rules file, `~/.brigadier/models.md`; a rule
 * holds everywhere or for one kind of work (a row's Work).
 */

/** What a new rule does: keeps a model out, allows only some models, or caps the effort. */
export type DraftEffect = "never" | "only" | "cap";

/**
 * A rule being written: what it does, to which model (never, only) or up to which effort (cap),
 * and for which kind of work (`null`: any work).
 */
export type Draft = { effect: DraftEffect; target: OverrideTarget | null; effort: string; work: string | null };

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

/** The names the rules' words need, from the model list. */
export function ruleNames(groups: readonly ModelGroup[]): Names {
  const pattern = (agent: ProviderKind | null, name: string) => {
    if (name === "*") return "any model";
    if (name === "default") return agent ? shortPlaceName({ type: "vendor", provider: agent }, groups) : "the default model";
    const listed = (agent ? groups.filter((group) => group.provider === agent) : groups)
      .map((group) => findModel(groups, { provider: group.provider, model: name, effort: null }))
      .find(Boolean);
    return listed?.displayName ?? name.charAt(0).toUpperCase() + name.slice(1);
  };
  return { pattern, target: (target) => shortPlaceName(target, groups) };
}

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

/** A rule as the file writes it: `forbid: fable`, `allow claude: opus, sonnet`, `effort-ceiling for Review: medium`. */
export function ruleText(rule: ModelRule): string {
  const scope = rule.work ? ` for ${rule.work}` : "";
  switch (rule.type) {
    case "forbid":
      return `forbid${scope}: ${rule.names.join(", ")}`;
    case "allow":
      return `allow${rule.agent ? ` ${rule.agent}` : ""}${scope}: ${rule.names.join(", ")}`;
    case "effortCeiling":
      return `effort-ceiling${scope}: ${rule.effort}`;
  }
}

/** "Never give Fable work", "Don't use Sonnet 5.5 for Review", "Only use Opus or Sonnet on Claude", "Never go above High effort". */
export function ruleTitle(rule: ModelRule, names: Names): string {
  const scope = rule.work ? ` for ${rule.work}` : "";
  switch (rule.type) {
    case "forbid": {
      const models = joinWords(rule.names.map((name) => names.pattern(null, name)), "or");
      return rule.work ? `Don't use ${models} for ${rule.work}` : `Never give ${models} work`;
    }
    case "allow": {
      const models = joinWords(rule.names.map((name) => names.pattern(rule.agent, name)), "or");
      return `Only use ${models}${rule.agent ? ` on ${VENDOR_LABELS[rule.agent]}` : ""}${scope}`;
    }
    case "effortCeiling":
      return `Never go above ${effortLabel(rule.effort)} effort${scope}`;
  }
}

/**
 * Where a rule applies, then what it does: "Everywhere · Chats, sessions and workers can't run
 * on it", "Review only · Its workers can't run on it".
 */
export function ruleLine(rule: ModelRule): string {
  const scope = rule.work ? `${rule.work} only` : "Everywhere";
  const vendor = rule.type === "allow" && rule.agent ? `${VENDOR_LABELS[rule.agent]} ` : "";
  const who = rule.work ? `Its ${vendor}workers` : vendor ? `${vendor}chats, sessions and workers` : "Chats, sessions and workers";
  switch (rule.type) {
    case "forbid":
      return `${scope} · ${who} can't run on ${rule.names.length === 1 ? "it" : "them"}`;
    case "allow":
      return `${scope} · ${who} run on nothing else`;
    case "effortCeiling":
      return `${scope} · ${who} run at this effort or lower`;
  }
}

/** Whether a rule holds for a start of `work` (`null`: a chat, a session, or a start for no work). */
const holdsFor = (rule: ModelRule, work: string | null) =>
  !rule.work || (work !== null && rule.work.trim().toLowerCase() === work.trim().toLowerCase());

/** A choice a rule may refuse: its agent, the names its model goes by, its effort. */
export type Choice = { provider: ProviderKind; names: readonly string[]; effort: string | null };

/** The first rule that refuses `choice` for a start of `work`, as the daemon judges it; `null` if none does. */
export function refusingRule(rules: readonly ModelRule[], choice: Choice, work: string | null): ModelRule | null {
  return (
    rules.find((rule) => {
      if (!holdsFor(rule, work)) return false;
      switch (rule.type) {
        case "forbid":
          return named(rule.names, choice.names);
        case "allow":
          return (rule.agent === null || rule.agent === choice.provider) && choice.names.length > 0 && !named(rule.names, choice.names);
        case "effortCeiling":
          return choice.effort !== null && effortRank(choice.effort) > effortRank(rule.effort);
      }
    }) ?? null
  );
}

/** A row as a choice: the names its model goes by (as written, and as the listed model it is); none for the CLI's default. */
export function rowChoice(row: GuidanceRow, models: readonly ListedModel[]): Choice {
  // The CLI's default: which model it is isn't known here, so no rule about a model judges it.
  if (row.model === "default") return { provider: row.agent, names: [], effort: row.effort };
  const listed = models.find((model) => model.provider === row.agent && (model.id === row.model || model.resolved === row.model));
  return {
    provider: row.agent,
    names: [row.model, ...(listed ? [listed.id, ...(listed.resolved ? [listed.resolved] : [])] : [])],
    effort: row.effort,
  };
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
 * they don't.
 */
export function pickRefusal(
  rules: readonly ModelRule[],
  names: Names,
  provider: ProviderKind,
  model: ModelInfo,
  effort: string | null,
): string | null {
  const rule = refusingRule(rules, modelPick(provider, model, effort), null);
  return rule ? ruleTitle(rule, names) : null;
}

/**
 * A pick the rules allow, as near the one asked for as can be: an effort above a ceiling comes
 * down to the highest of its model's efforts the rules allow. Any other pick stays as it is.
 */
export function allowedPick(rules: readonly ModelRule[], groups: readonly ModelGroup[], choice: ModelChoice): ModelChoice {
  const model = findModel(groups, choice);
  if (!model || rules.length === 0) return choice;
  const rule = refusingRule(rules, pickedChoice(groups, choice), null);
  if (rule?.type !== "effortCeiling") return choice;
  const lower = model.efforts.toReversed().find((effort) => !refusingRule(rules, pickedChoice(groups, { ...choice, effort }), null));
  return lower ? { ...choice, effort: lower } : choice;
}

/** A rules-file line as the daemon quotes it in a refusal, compared without case or spaces. */
const lineKey = (line: string) => line.toLowerCase().replace(/\s+/g, "");

/**
 * An error in the Rules card's words when it is the daemon's refusal by one of the user's rules
 * (`refused by your model rule: "effort-ceiling: high"`): "Refused by your rule “Never go above
 * High effort”". Any other error stays as it is.
 */
export function refusalText(message: string, rules: readonly ModelRule[], names: Names): string {
  const quoted = /refused by your model rule: "(.*)"/s.exec(message)?.[1];
  if (quoted === undefined) return message;
  const rule = rules.find((found) => lineKey(ruleText(found)) === lineKey(quoted));
  return rule ? `Refused by your rule “${ruleTitle(rule, names)}”` : message;
}

/** The word of a draft's model the rule names: a listed id, a family word, or `default`. */
function draftName(target: OverrideTarget): string {
  return target.type === "model" ? target.id : target.type === "family" ? target.family : "default";
}

/** Whether two rules hold for the same work. */
const sameScope = (rule: ModelRule, work: string | null) => (rule.work ?? null) === work;

/**
 * The rules once a finished draft is added: Never adds its own line; Only joins the allowlist
 * of the model's agent for the same work when there is one (two allowlists would allow only
 * what both name); a ceiling replaces the one of the same work.
 */
export function draftRules(draft: Draft, rules: readonly ModelRule[]): ModelRule[] {
  const scope = draft.work ? { work: draft.work } : {};
  if (draft.effect === "cap") {
    const ceiling: ModelRule = { type: "effortCeiling", effort: draft.effort, ...scope };
    const at = rules.findIndex((rule) => rule.type === "effortCeiling" && sameScope(rule, draft.work));
    return at < 0 ? [...rules, ceiling] : rules.map((rule, index) => (index === at ? ceiling : rule));
  }
  const target = draft.target;
  if (!target) return [...rules];
  const name = draftName(target);
  if (draft.effect === "never") return [...rules, { type: "forbid", names: [name], ...scope }];
  const own = rules.findIndex((rule) => rule.type === "allow" && rule.agent === target.provider && sameScope(rule, draft.work));
  if (own < 0) return [...rules, { type: "allow", agent: target.provider, names: [name], ...scope }];
  return rules.map((rule, index) =>
    index === own && rule.type === "allow" && !rule.names.includes(name) ? { ...rule, names: [...rule.names, name] } : rule,
  );
}

/**
 * What a draft would change, said before it is added; a warning when it would change nothing
 * or clash. `works`, the kinds of work it would stop from starting, for the note to open.
 */
export type DraftPreview = { tone: "info" | "warning"; text: string; works?: string[] };

/** "“Bounded implementation” and “Review”": the kinds of work rows name. */
const works = (rows: readonly WorkRow[]) => joinWords(rows.map((row) => `“${row.work}”`));

/**
 * What adding the draft would do, from the models the agents list now and the rows in force;
 * a warning when it is already a rule, Never already keeps its model out, or a kind of work
 * that starts now could no longer start (then where to fix it).
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
  const target = draft.target;
  if (draft.effect !== "cap" && !target) return null;
  const next = draftRules(draft, rules);
  const made = next.find((rule) => !rules.some((old) => ruleText(old) === ruleText(rule)));
  const kept = (rule: ModelRule) => rule.type === "forbid" && sameScope(rule, draft.work) && target !== null && rule.names.includes(draftName(target));
  if (!made || (draft.effect === "never" && rules.some(kept))) return { tone: "warning", text: "You already have this rule." };
  const scope = draft.work ? ` for ${draft.work}` : "";
  // A kind of work starts while any of its rows may: the first its rules allow takes it.
  const starts = (row: WorkRow, using: readonly ModelRule[]) =>
    row.rows.some((found) => !refusingRule(using, rowChoice(found, models), row.work));
  const inScope = rows.filter((row) => draft.work === null || row.work === draft.work);
  const hit = inScope.filter((row) => starts(row, rules) && !starts(row, next));
  const them = hit.length === 1 ? "it" : "them";
  const fix = (change: string) =>
    `${change} in Who does what${draft.work ? "" : ", or make this rule for one kind of work"}.`;
  if (draft.effect === "cap") {
    if (hit.length > 0) {
      return {
        tone: "warning",
        text: `${works(hit)} ${hit.length === 1 ? "asks" : "ask"} for more than ${effortLabel(draft.effort)} effort, so starting ${them} would be refused. ${fix(`Lower ${hit.length === 1 ? "its" : "their"} effort`)}`,
        works: hit.map((row) => row.work),
      };
    }
    const old = rules.find((rule) => rule.type === "effortCeiling" && sameScope(rule, draft.work));
    const replaces = old?.type === "effortCeiling" ? ` It replaces your ${effortLabel(old.effort)} ceiling${scope}.` : "";
    const who = draft.work ? `Workers for ${draft.work}` : "Chats, sessions and workers";
    return { tone: "info", text: `${who} will run at ${effortLabel(draft.effort)} effort or lower; asking for more is refused.${replaces}` };
  }
  if (!target) return null;
  const name = draftName(target);
  const model = names.target(target);
  const vendor = VENDOR_LABELS[target.provider];
  if (draft.effect === "only") {
    // Never rules win: Only of a model a Never rule keeps out leaves no model of the agent.
    const never = rules.find((rule) => rule.type === "forbid" && holdsFor(rule, draft.work) && named(rule.names, [name]));
    if (never) {
      return {
        tone: "warning",
        text: `“${ruleTitle(never, names)}” already keeps ${model} out, and Never rules win, so with this rule no ${vendor} model could run${scope}. Remove that rule first.`,
      };
    }
  }
  if (hit.length > 0) {
    const uses = hit.length === 1 ? "uses" : "use";
    const what = draft.effect === "never" ? model : `another ${vendor} model`;
    return {
      tone: "warning",
      text: `${works(hit)} ${uses} ${what}, so starting ${them} would be refused. ${fix(`Pick another model for ${them}`)}`,
      works: hit.map((row) => row.work),
    };
  }
  if (draft.effect === "never") {
    const caught = models.filter((found) => named([name], [found.id, ...(found.resolved ? [found.resolved] : [])]));
    if (caught.length === 0) return { tone: "info", text: `No model listed now is ${model}; the rule applies once one is.` };
    const who = draft.work ? `a worker for ${draft.work}` : "a chat, session or worker";
    return {
      tone: "info",
      text: `${joinWords(caught.map((found) => found.name))} won't be used${scope}: ${who} that asks for ${caught.length === 1 ? "it" : "them"} is refused, quoting this rule.`,
    };
  }
  const allow = next.find((rule) => rule.type === "allow" && rule.agent === target.provider && sameScope(rule, draft.work));
  const allowed = allow?.type === "allow" ? allow.names : [name];
  const joins = allowed.length > 1 ? `${model} joins your Only rule for ${vendor}${scope}. ` : "";
  const refused = models.filter(
    (found) => found.provider === target.provider && !named(allowed, [found.id, ...(found.resolved ? [found.resolved] : [])]),
  );
  const rest =
    refused.length === 0
      ? `No ${vendor} model listed now is refused.`
      : `${joinWords(refused.map((found) => found.name))} ${refused.length === 1 ? "is" : "are"} refused.`;
  const who = draft.work ? `${vendor} workers for ${draft.work}` : `${vendor} chats, sessions and workers`;
  return { tone: "info", text: `${joins}${who} will use only ${joinWords(allowed.map((found) => names.pattern(target.provider, found)), "or")}. ${rest}` };
}
