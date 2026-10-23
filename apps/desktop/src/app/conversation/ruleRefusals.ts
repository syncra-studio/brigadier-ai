import { useMemo } from "react";

import { allowedPick, pickRefusal, refusalText, ruleNames } from "@/app/settings/routing/ruleWords";
import type { ModelGroup } from "@/components/shared/model-selector";
import type { ModelChoice, ModelInfo, ModelRule, ProviderKind } from "@/ipc/generated";
import { useModelRules } from "@/state/modelRules";

const NO_RULES: ModelRule[] = [];

/** The user's rules (`~/.brigadier/models.md`, as Settings › Routing reads it) as the composer uses them. */
export type RuleRefusals = {
  /** Why the rules keep a model, or one of its efforts, out; `null` if they don't. */
  refused: (provider: ProviderKind, model: ModelInfo, effort: string | null) => string | null;
  /** The pick with an effort above a ceiling brought down to one the rules allow. */
  allowed: (choice: ModelChoice) => ModelChoice;
  /** An error in plain words when it is a refusal by one of the rules. */
  words: (message: string) => string;
};

/** The rules for the composer's pickers and its banners, named from `groups`. */
export function useRuleRefusals(groups: readonly ModelGroup[]): RuleRefusals {
  const rules = useModelRules((s) => s.view?.rules ?? NO_RULES);
  return useMemo(() => {
    const names = ruleNames(groups);
    return {
      refused: (provider, model, effort) => pickRefusal(rules, names, provider, model, effort),
      allowed: (choice) => allowedPick(rules, groups, choice),
      words: (message) => refusalText(message, rules, names),
    };
  }, [rules, groups]);
}
