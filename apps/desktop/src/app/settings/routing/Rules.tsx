import { useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { BanIcon, Badge, Note, PreferIcon } from "@/app/settings/routing/parts";
import { TargetItems } from "@/app/settings/routing/pickers";
import {
  type Draft,
  type DraftEffect,
  draftRules,
  EFFORT_LEVELS,
  type ListedModel,
  type Names,
  previewDraft,
  ruleLine,
  ruleText,
  ruleTitle,
} from "@/app/settings/routing/ruleWords";
import { shortPlaceName, workRows } from "@/app/settings/routing/words";
import { SettingsSection } from "@/app/settings/parts";
import { effortLabel, findModel, type ModelGroup, SettingsCard } from "@/components/shared";
import { Button, SettingsSelect, SettingsSelectTrigger } from "@/components/controls";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/overlays";
import { DotsHorizontal, Info, Plus, Warning } from "@/components/icons";
import type { ModelRule, ModelRules, OverrideTarget, ProviderKind } from "@/ipc/generated";
import { saveModelRules } from "@/state/modelRules";
import { toast } from "@/state/toasts";
import { useUsage } from "@/state/usage";

export const RULES = {
  label: "Rules",
  description: "Keep a model out, allow only some models, or cap the effort",
} as const;

const EFFECTS: { value: DraftEffect; label: string }[] = [
  { value: "never", label: "Never" },
  { value: "only", label: "Only" },
  { value: "cap", label: "Cap effort" },
];

/** The efforts a ceiling offers: those an agent runs at. */
const CEILINGS = EFFORT_LEVELS.filter((level) => level !== "none" && level !== "ultra").map((level) => ({
  value: level as string,
  label: effortLabel(level),
}));

/** The names the rules' words need, from the model list. */
function useNames(groups: readonly ModelGroup[]): Names {
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

/**
 * Rules: each as a sentence with where it applies, removed from its menu. Add rule opens a
 * sentence to complete at the top of the card, which says what it would change before it is
 * added; an added rule is marked New and a toast offers Undo.
 */
export function Rules({ rules: file, groups }: { rules: ModelRules | null; groups: readonly ModelGroup[] }) {
  const rules = file?.rules ?? [];
  const [adding, setAdding] = useState(false);
  const [fresh, setFresh] = useState<string | null>(null);
  const remove = useAction();
  const names = useNames(groups);
  const save = (next: readonly ModelRule[]) => saveModelRules(file?.rows ?? [], next);
  const added = rules.find((rule) => ruleText(rule) === fresh);
  // The rule just added leads the list while it is new.
  const shown = added ? [added, ...rules.filter((rule) => rule !== added)] : rules;
  const onAdded = (rule: ModelRule | undefined, before: readonly ModelRule[]) => {
    setAdding(false);
    setFresh(rule ? ruleText(rule) : null);
    toast("Rule added", { actions: [{ label: "Undo", run: () => void save(before) }] });
  };
  return (
    <SettingsSection
      title={RULES.label}
      description={RULES.description}
      actions={
        <Button variant="soft" size="sm" data-slot="add-rule" disabled={adding || !file} onClick={() => setAdding(true)}>
          <Plus aria-hidden />
          Add rule
        </Button>
      }
    >
      {file && (shown.length > 0 || adding) && (
        <SettingsCard inset data-slot="routing-rules">
          {adding && <DraftRow file={file} names={names} onCancel={() => setAdding(false)} onAdded={onAdded} />}
          {shown.map((rule) => (
            <div
              key={ruleText(rule)}
              data-slot="routing-rule"
              data-setting={ruleTitle(rule, names)}
              className="flex items-center gap-3.5 py-3.5">
              {rule.type === "allow" ? <PreferIcon /> : <BanIcon />}
              <span className="flex min-w-0 flex-1 flex-col gap-1">
                <span className="flex min-w-0 items-center gap-2">
                  <span className="text-label text-foreground truncate font-medium">{ruleTitle(rule, names)}</span>
                  {rule === added && <Badge tone="link">New</Badge>}
                </span>
                <span className="text-xs text-foreground/65 truncate">{ruleLine(rule)}</span>
              </span>
              <DropdownMenu modal={false}>
                <DropdownMenuTrigger asChild>
                  <button
                    type="button"
                    aria-label={`${ruleTitle(rule, names)}: actions`}
                    className="text-muted-foreground hover:text-foreground flex shrink-0 items-center"
                  >
                    <DotsHorizontal aria-hidden className="size-3.75" />
                  </button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end">
                  <DropdownMenuItem
                    disabled={remove.busy}
                    onSelect={() => remove.run(() => save(rules.filter((other) => other !== rule)))}
                  >
                    Remove
                  </DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
            </div>
          ))}
        </SettingsCard>
      )}
      {remove.error && (
        <p role="alert" className="text-destructive text-xs">
          {remove.error}
        </p>
      )}
    </SettingsSection>
  );
}

/** A new rule as a sentence to complete: "[Never] use [a model]", "[Cap effort] at [High]", then what it would change. */
function DraftRow({
  file,
  names,
  onCancel,
  onAdded,
}: {
  file: ModelRules;
  names: Names;
  onCancel: () => void;
  onAdded: (rule: ModelRule | undefined, before: readonly ModelRule[]) => void;
}) {
  const view = useUsage((s) => s.view);
  const [draft, setDraft] = useState<Draft>({ effect: "never", target: null, effort: "high" });
  const add = useAction();
  const target = draft.target;
  const capping = draft.effect === "cap";
  const models: ListedModel[] = (view?.models ?? []).map((model) => ({ provider: model.provider, id: model.id, resolved: model.resolved, name: model.displayName }));
  const preview = previewDraft(draft, { rules: file.rules, rows: workRows(file), models, names });
  const submit = () => {
    if (!capping && !target) return;
    const next = draftRules(draft, file.rules);
    // The rule the draft made or grew: the one not in the file as it was.
    const made = next.find((rule) => !file.rules.some((old) => ruleText(old) === ruleText(rule)));
    add.run(async () => {
      await saveModelRules(file.rows, next);
      onAdded(made, file.rules);
    });
  };
  const set = (change: Partial<Draft>) => setDraft((current) => ({ ...current, ...change }));
  return (
    <div data-slot="routing-rule-form" className="flex flex-col gap-3 py-4">
      <div className="flex items-center gap-3.5">
        {draft.effect === "only" ? <PreferIcon /> : <BanIcon />}
        <div className="text-label text-foreground/65 flex min-w-0 flex-1 flex-wrap items-center gap-2">
          <SettingsSelect align="start" label="What the rule does" value={draft.effect} options={EFFECTS} onChange={(effect) => set({ effect })} />
          {capping ? (
            <>
              at
              <SettingsSelect align="start" label="The highest effort" value={draft.effort} options={CEILINGS} onChange={(effort) => set({ effort })} />
            </>
          ) : (
            <>
              use
              <DropdownMenu modal={false}>
                <DropdownMenuTrigger asChild>
                  <SettingsSelectTrigger label="The model the rule is about" placeholder={!target}>
                    {target ? names.target(target) : "Choose a model"}
                  </SettingsSelectTrigger>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="start" className="max-h-96 w-xs overflow-y-auto">
                  <TargetItems onPick={(picked: OverrideTarget) => set({ target: picked })} vendors={false} />
                </DropdownMenuContent>
              </DropdownMenu>
            </>
          )}
        </div>
        <Button variant="ghost" size="sm" onClick={onCancel}>Cancel</Button>
        <Button size="sm" disabled={(!capping && !target) || add.busy} onClick={submit}>
          Add rule
        </Button>
      </div>
      {preview && (
        <Note tone={preview.tone} icon={preview.tone === "info" ? <Info aria-hidden /> : <Warning aria-hidden />}>
          {preview.text}
        </Note>
      )}
      {add.error && (
        <p role="alert" className="text-destructive text-xs">
          {add.error}
        </p>
      )}
    </div>
  );
}
