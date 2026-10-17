import { DotsHorizontal, Plus, Sparkles } from "@openai/apps-sdk-ui/components/Icon";
import { useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { ChipSelect, TargetSelect, useScopeOptions } from "@/app/routing/pickers";
import { ROUTED_KINDS, shortPlaceName } from "@/app/routing/words";
import { SettingsButton, SettingsCard, SettingsRow, SettingsSection } from "@/app/settings/parts";
import type { ModelGroup } from "@/components/assistant-ui/elements/model-selector";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import type { OverrideEffect, OverrideRule, OverrideTarget, Ranking, TaskCategory } from "@/ipc/generated";
import {
  AREA_LABELS,
  isNoWorkRule,
  joinWords,
  newRuleId,
  noWorkRule,
  PLAIN_KIND_NAMES,
  PLAIN_KIND_WORDS,
  RANKED_CATEGORIES,
  ruleSentence,
} from "@/lib/routing";
import { isNewModelRule } from "@/state/providers";
import { addOverride, removeOverride } from "@/state/routing";
import { useApp } from "@/state/store";

/** What a rule does: "Don't give work" is a `never` for every worker kind of work, everywhere. */
type Effect = OverrideEffect | "noWork";

const EFFECT_OPTIONS: { value: Effect; label: string }[] = [
  { value: "never", label: "Never use" },
  { value: "noWork", label: "Don't give work to" },
  { value: "prefer", label: "Prefer" },
  { value: "only", label: "Only use" },
];

/**
 * Where a `prefer` rule does nothing: the kinds of work it covers whose own row has a list in
 * its scope (a list decides there, not preferences).
 */
function shadowedBy(rule: OverrideRule, rankings: readonly Ranking[]): string | null {
  if (rule.effect !== "prefer") return null;
  const covered = rule.categories.length > 0 ? rule.categories : RANKED_CATEGORIES;
  const listed = covered.filter((category) =>
    rankings.some(
      (ranking) =>
        ranking.manual &&
        ranking.category === category &&
        ranking.areas.length === 0 &&
        ranking.entries.length > 0 &&
        (ranking.projectId === rule.projectId || ranking.projectId === null),
    ),
  );
  if (listed.length === 0) return null;
  return `Not used for ${joinWords(listed.map((category) => PLAIN_KIND_WORDS[category]))}: your order decides there`;
}

/**
 * The rules, each as a plain sentence ("Never use Sonnet 5.5 for reviewing changes") with where
 * it applies and what it leaves, removed from its menu; Add rule opens a sentence to fill in,
 * about the kind of work opened above when one is.
 */
export function RoutingRules({
  category,
  scope,
  groups,
}: {
  category: TaskCategory | null;
  scope: string | null;
  groups: readonly ModelGroup[];
}) {
  const rules = useApp((s) => s.settings.routingOverrides);
  const rankings = useApp((s) => s.settings.routingRankings);
  const projects = useApp((s) => s.projects);
  const [adding, setAdding] = useState(false);
  const remove = useAction();
  return (
    <SettingsSection
      title="Rules"
      description="Models to keep from some work, to prefer, or to use only"
      actions={
        !adding && (
          <SettingsButton onClick={() => setAdding(true)}>
            <Plus />
            Add rule
          </SettingsButton>
        )
      }
    >
      <div data-setting="Rules" data-slot="routing-rules" className="flex flex-col gap-2">
        {(rules.length > 0 || adding) && (
          <SettingsCard>
            {rules.map((rule) => {
              const sentence = ruleSentence(rule, groups, projects);
              const noWork = isNoWorkRule(rule);
              const what = noWork
                ? `Don't give ${shortPlaceName(rule.target, groups)} work`
                : [
                    EFFECT_OPTIONS.find((option) => option.value === rule.effect)?.label,
                    shortPlaceName(rule.target, groups),
                    rule.categories.length > 0 || rule.areas.length > 0 ? "for" : null,
                    joinWords(rule.areas.map((area) => AREA_LABELS[area]), "or"),
                    joinWords(rule.categories.map((kind) => PLAIN_KIND_WORDS[kind]), "or"),
                  ]
                    .filter(Boolean)
                    .join(" ");
              const where = rule.projectId ? `In ${projects[rule.projectId]?.name ?? "a removed project"}` : "Everywhere";
              const note = noWork
                ? isNewModelRule(rule)
                  ? "A new model: it gets no tasks until you remove this rule"
                  : "You can still pick it for a chat or session"
                : where;
              const shadowed = shadowedBy(rule, rankings);
              return (
                <SettingsRow
                  key={rule.id}
                  setting={sentence}
                  label={what}
                  description={shadowed ? `${note}. ${shadowed}` : note}
                >
                  <DropdownMenu modal={false}>
                    <DropdownMenuTrigger asChild>
                      <Button
                        type="button"
                        size="icon-xs"
                        variant="ghost"
                        aria-label={`${sentence}: actions`}
                        className="text-muted-foreground shrink-0"
                      >
                        <DotsHorizontal />
                      </Button>
                    </DropdownMenuTrigger>
                    <DropdownMenuContent align="end">
                      <DropdownMenuItem
                        disabled={remove.busy}
                        onSelect={() => remove.run(() => removeOverride(rule.id))}
                      >
                        Remove
                      </DropdownMenuItem>
                    </DropdownMenuContent>
                  </DropdownMenu>
                </SettingsRow>
              );
            })}
            {adding && <NewRule category={category} scope={scope} groups={groups} onDone={() => setAdding(false)} />}
          </SettingsCard>
        )}
        {rules.length === 0 && !adding && (
          <p className="text-muted-foreground text-xs">No rules: only Brigadier's own limits apply.</p>
        )}
        {remove.error && (
          <p role="alert" className="text-destructive text-xs">
            {remove.error}
          </p>
        )}
      </div>
    </SettingsSection>
  );
}

/** A new rule as a sentence to fill in: "[Never use] [a model] for [Review changes], [in this project]." */
function NewRule({
  category,
  scope,
  groups,
  onDone,
}: {
  category: TaskCategory | null;
  scope: string | null;
  groups: readonly ModelGroup[];
  onDone: () => void;
}) {
  const scopes = useScopeOptions();
  const [effect, setEffect] = useState<Effect>("never");
  const [target, setTarget] = useState<OverrideTarget | null>(null);
  const [kind, setKind] = useState<TaskCategory | "">(category ?? "");
  const [projectId, setProjectId] = useState<string>(scope ?? "");
  const add = useAction();
  const noWork = effect === "noWork";
  const submit = () => {
    if (!target) return;
    const rule: OverrideRule = noWork
      ? noWorkRule(target, newRuleId(), Date.now())
      : {
          id: newRuleId(),
          effect,
          target,
          categories: kind === "" ? [] : [kind],
          areas: [],
          projectId: projectId === "" ? null : projectId,
          createdAtMs: Date.now(),
        };
    add.run(async () => {
      await addOverride(rule);
      onDone();
    });
  };
  return (
    <div
      data-slot="routing-rule-form"
      className="flex flex-col gap-2.5 px-4 py-3"
    >
      <span className="text-muted-foreground flex items-center gap-1.5 text-2xs font-medium">
        <Sparkles aria-hidden className="size-icon-xs" />
        New rule · preview
      </span>
      <div className="flex flex-wrap items-center gap-2 text-sm">
        <ChipSelect label="What the rule does" value={effect} options={EFFECT_OPTIONS} onChange={setEffect} />
        <TargetSelect
          label="The model the rule is about"
          value={target}
          name={target ? shortPlaceName(target, groups) : ""}
          onChange={setTarget}
        />
        {noWork ? (
          <span className="text-muted-foreground text-xs">— you can still pick it for a chat or session.</span>
        ) : (
          <>
            for
            <ChipSelect
              label="The kind of work the rule is about"
              value={kind}
              options={[
                ...ROUTED_KINDS.map((value) => ({ value, label: PLAIN_KIND_NAMES[value] })),
                { value: "" as const, label: "any kind of work" },
              ]}
              onChange={setKind}
            />
            ,
            <ChipSelect
              label="Where the rule applies"
              value={projectId}
              options={scopes.map((option) =>
                option.value === "" ? { value: "", label: "everywhere" } : { ...option, label: `in ${option.label}` },
              )}
              onChange={setProjectId}
            />
            .
          </>
        )}
        <span className="ms-auto flex gap-2">
          <SettingsButton onClick={onDone}>Cancel</SettingsButton>
          <Button type="button" size="sm" className="rounded-nav" disabled={!target || add.busy} onClick={submit}>
            Add
          </Button>
        </span>
      </div>
      {add.error && (
        <p role="alert" className="text-destructive text-xs">
          {add.error}
        </p>
      )}
    </div>
  );
}
