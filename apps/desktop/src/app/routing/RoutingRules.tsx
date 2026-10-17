import { ArrowUp, Check, DotsHorizontal, MinusCircle, Plus, Sparkles } from "@openai/apps-sdk-ui/components/Icon";
import { type ReactNode, useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { ChipSelect, TargetSelect, useScopeOptions } from "@/app/routing/pickers";
import { ROUTED_KINDS, shortPlaceName } from "@/app/routing/words";
import { SettingsButton } from "@/app/settings/parts";
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
  WORKER_CATEGORIES,
} from "@/lib/routing";
import { cn } from "@/lib/utils";
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

const EFFECT_ICONS: Record<Effect, typeof MinusCircle> = {
  never: MinusCircle,
  noWork: MinusCircle,
  prefer: ArrowUp,
  only: Check,
};

/** Whether a rule is about a kind of work (one naming none is about every kind). */
export function ruleCovers(rule: OverrideRule, category: TaskCategory): boolean {
  if (isNoWorkRule(rule)) return WORKER_CATEGORIES.includes(category);
  return rule.categories.length === 0 || rule.categories.includes(category);
}

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

/** A word of a rule's sentence, set apart. */
function Word({ children }: { children: ReactNode }) {
  return (
    <span className="border-divider bg-foreground/5 rounded-nav text-label inline-flex h-7 items-center border px-2.5">
      {children}
    </span>
  );
}

/**
 * The rules about a kind of work, each as a sentence ("Never use [Sonnet 5.5] for [reviewing
 * changes], [everywhere]."), removed from its menu; Add rule opens a sentence to fill in.
 */
export function RoutingRules({
  category,
  scope,
  groups,
}: {
  category: TaskCategory;
  scope: string | null;
  groups: readonly ModelGroup[];
}) {
  const rules = useApp((s) => s.settings.routingOverrides);
  const rankings = useApp((s) => s.settings.routingRankings);
  const projects = useApp((s) => s.projects);
  const [adding, setAdding] = useState(false);
  const remove = useAction();
  const shown = rules.filter((rule) => ruleCovers(rule, category));
  return (
    <section data-setting="Rules" data-slot="routing-rules" className="flex flex-col gap-2">
      <div className="flex items-center justify-between gap-2">
        <h3 className="text-sm font-medium">Rules</h3>
        {!adding && (
          <SettingsButton onClick={() => setAdding(true)}>
            <Plus />
            Add rule
          </SettingsButton>
        )}
      </div>
      {shown.length === 0 && !adding && (
        <p className="text-muted-foreground text-xs">
          No rules about {PLAIN_KIND_WORDS[category]}: only Brigadier's own limits apply.
        </p>
      )}
      {shown.map((rule) => {
        const sentence = ruleSentence(rule, groups, projects);
        const noWork = isNoWorkRule(rule);
        const Icon = EFFECT_ICONS[noWork ? "noWork" : rule.effect];
        const where = rule.projectId ? `in ${projects[rule.projectId]?.name ?? "a removed project"}` : "everywhere";
        const note = noWork
          ? isNewModelRule(rule)
            ? " — a new model: it gets no tasks until you remove this rule."
            : " — you can still pick it for a chat or session."
          : ".";
        const shadowed = shadowedBy(rule, rankings);
        return (
          <div
            key={rule.id}
            data-setting={sentence}
            data-slot="routing-rule"
            className="bg-card border-divider rounded-settings flex items-center gap-2 border px-4 py-2.5 text-sm"
          >
            <Icon
              aria-hidden
              className={cn(
                "size-icon-sm shrink-0",
                rule.effect === "never" ? "text-destructive" : "text-muted-foreground",
              )}
            />
            <span className="flex min-w-0 flex-1 flex-wrap items-center gap-x-1.5 gap-y-1">
              <span className="sr-only">{sentence}</span>
              <span aria-hidden className="contents">
                {noWork ? "Don't give work to" : EFFECT_OPTIONS.find((option) => option.value === rule.effect)?.label}
                <Word>{shortPlaceName(rule.target, groups)}</Word>
                {!noWork && (rule.categories.length > 0 || rule.areas.length > 0) && (
                  <>
                    for
                    <Word>
                      {[
                        joinWords(rule.areas.map((area) => AREA_LABELS[area]), "or"),
                        joinWords(rule.categories.map((kind) => PLAIN_KIND_WORDS[kind]), "or"),
                      ]
                        .filter(Boolean)
                        .join(" ")}
                    </Word>
                    ,
                  </>
                )}
                <Word>{where}</Word>
                <span className="text-muted-foreground text-xs">{note}</span>
              </span>
              {shadowed && <span className="text-muted-foreground basis-full text-xs">{shadowed}</span>}
            </span>
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
          </div>
        );
      })}
      {remove.error && (
        <p role="alert" className="text-destructive text-xs">
          {remove.error}
        </p>
      )}
      {adding && <NewRule category={category} scope={scope} groups={groups} onDone={() => setAdding(false)} />}
    </section>
  );
}

/** A new rule as a sentence to fill in: "[Never use] [a model] for [Review changes], [in this project]." */
function NewRule({
  category,
  scope,
  groups,
  onDone,
}: {
  category: TaskCategory;
  scope: string | null;
  groups: readonly ModelGroup[];
  onDone: () => void;
}) {
  const scopes = useScopeOptions();
  const [effect, setEffect] = useState<Effect>("never");
  const [target, setTarget] = useState<OverrideTarget | null>(null);
  const [kind, setKind] = useState<TaskCategory | "">(category);
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
      className="border-divider rounded-settings flex flex-col gap-2.5 border border-dashed px-4 py-3"
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
