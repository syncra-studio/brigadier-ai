import { Plus, Trash } from "@openai/apps-sdk-ui/components/Icon";
import { useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { RuleForm } from "@/app/routing/RuleForm";
import { SettingsButton, SettingsCard, SettingsRow, SettingsSection } from "@/app/settings/parts";
import type { ModelGroup } from "@/components/assistant-ui/elements/model-selector";
import { Button } from "@/components/ui/button";
import type { OverrideRule, Ranking } from "@/ipc/generated";
import {
  CATEGORY_LABELS,
  isNoWorkRule,
  joinWords,
  RANKED_CATEGORIES,
  ruleSentence,
} from "@/lib/routing";
import { isNewModelRule } from "@/state/providers";
import { removeOverride } from "@/state/routing";
import { useApp } from "@/state/store";

/** The rules section's heading, for the page and Settings search. */
export const RULES_ROW = {
  label: "Rules",
  description:
    "Don't give work keeps a model from tasks but lets you pick it. Never and Only keep models from work, whether a row is Automatic or Manual. Prefer puts a model first where routing scores.",
} as const;

/** What a "Don't give work" rule leaves the user, on its row. */
function noWorkNote(rule: OverrideRule): string | null {
  if (!isNoWorkRule(rule)) return null;
  return isNewModelRule(rule)
    ? "New model: Brigadier gives it no tasks until you remove this rule."
    : "You can still pick it for a chat or session.";
}

/**
 * Where a `prefer` rule does nothing: the kinds of work it covers whose own row is Manual in its
 * scope (a Manual list decides there, not preferences).
 */
function shadowedBy(rule: OverrideRule, rankings: readonly Ranking[]): string | null {
  if (rule.effect !== "prefer") return null;
  const covered = rule.categories.length > 0 ? rule.categories : RANKED_CATEGORIES;
  const manual = covered.filter((category) =>
    rankings.some(
      (ranking) =>
        ranking.manual &&
        ranking.category === category &&
        ranking.areas.length === 0 &&
        ranking.entries.length > 0 &&
        (ranking.projectId === rule.projectId || ranking.projectId === null),
    ),
  );
  if (manual.length === 0) return null;
  return `Not used for ${joinWords(manual.map((category) => CATEGORY_LABELS[category]))}: your list decides there.`;
}

/**
 * The user's rules: "Don't give work" keeps a model from tasks (a chat or session may still run
 * on it), `never` and `only` keep models from work (Automatic and Manual alike), `prefer` puts
 * one first where routing scores. Adding or removing one applies at once.
 */
export function RulesSection({ groups }: { groups: readonly ModelGroup[] }) {
  const rules = useApp((s) => s.settings.routingOverrides);
  const rankings = useApp((s) => s.settings.routingRankings);
  const projects = useApp((s) => s.projects);
  const [adding, setAdding] = useState(false);
  const remove = useAction();
  return (
    <SettingsSection
      title={RULES_ROW.label}
      description={RULES_ROW.description}
      actions={
        !adding && (
          <SettingsButton onClick={() => setAdding(true)}>
            <Plus />
            Add rule
          </SettingsButton>
        )
      }
    >
      {adding && (
        <RuleForm
          groups={groups}
          onDone={() => setAdding(false)}
          className="rounded-settings border-divider p-4"
        />
      )}
      <SettingsCard>
        {rules.length === 0 ? (
          <SettingsRow label="No rules" description="Routing is limited only by Brigadier's own rules." />
        ) : (
          rules.map((rule) => {
            const sentence = ruleSentence(rule, groups, projects);
            return (
              <SettingsRow
                key={rule.id}
                label={sentence}
                description={noWorkNote(rule) ?? shadowedBy(rule, rankings)}
              >
                <Button
                  type="button"
                  size="icon-xs"
                  variant="ghost"
                  aria-label={`Remove: ${sentence}`}
                  className="text-muted-foreground"
                  disabled={remove.busy}
                  onClick={() => remove.run(() => removeOverride(rule.id))}
                >
                  <Trash />
                </Button>
              </SettingsRow>
            );
          })
        )}
      </SettingsCard>
      {remove.error && (
        <p role="alert" className="text-destructive text-xs">
          {remove.error}
        </p>
      )}
    </SettingsSection>
  );
}
