import { useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { BanIcon, Badge, Note, PreferIcon } from "@/app/settings/routing/parts";
import { TargetPicker, useListedModels } from "@/app/settings/routing/pickers";
import {
  type Draft,
  type DraftEffect,
  draftRules,
  EFFORT_LEVELS,
  type Names,
  previewDraft,
  ruleLine,
  ruleNames,
  ruleText,
  ruleTitle,
} from "@/app/settings/routing/ruleWords";
import { workName, workRows } from "@/app/settings/routing/words";
import { SettingsSection } from "@/app/settings/parts";
import { effortLabel, type ModelGroup, SettingsCard } from "@/components/shared";
import { Button, SettingsSelect, SettingsSelectTrigger } from "@/components/controls";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/overlays";
import { DotsHorizontal, Info, Plus, Warning } from "@/components/icons";
import type { ModelRule, ModelRules } from "@/ipc/generated";
import { saveModelRules } from "@/state/modelRules";
import { toast } from "@/state/toasts";

export const RULES = {
  label: "Rules",
  description: "Keep a model away from all work or one kind of work, allow only some models, or cap the effort",
} as const;

/** The "for" select's choice of every kind of work. */
const ANY_WORK = "";

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

/**
 * Rules: each as a sentence naming its model, then where it applies and what it does; removed
 * from its menu. Add rule opens a sentence to complete at the top of the card ("Never use [a
 * model] for [any work]"), which says what it would change before it is added and opens a kind
 * of work it would stop from starting; an added rule is marked New and a toast offers Undo.
 */
export function Rules({
  rules: file,
  groups,
  onOpenKind,
}: {
  rules: ModelRules | null;
  groups: readonly ModelGroup[];
  /** Opens a kind of work in Who does what. */
  onOpenKind: (work: string) => void;
}) {
  const rules = file?.rules ?? [];
  const [adding, setAdding] = useState(false);
  const [fresh, setFresh] = useState<string | null>(null);
  const remove = useAction();
  const names = ruleNames(groups);
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
          {adding && (
            <DraftRow file={file} names={names} onCancel={() => setAdding(false)} onAdded={onAdded} onOpenKind={onOpenKind} />
          )}
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

/**
 * A new rule as a sentence to complete: "[Never] use [a model] for [Any work]", "[Cap effort] at
 * [High] for [Any work]", then what it would change.
 */
function DraftRow({
  file,
  names,
  onCancel,
  onAdded,
  onOpenKind,
}: {
  file: ModelRules;
  names: Names;
  onCancel: () => void;
  onAdded: (rule: ModelRule | undefined, before: readonly ModelRule[]) => void;
  onOpenKind: (work: string) => void;
}) {
  const models = useListedModels();
  const [draft, setDraft] = useState<Draft>({ effect: "never", target: null, effort: "high", work: null });
  const add = useAction();
  const target = draft.target;
  const capping = draft.effect === "cap";
  const kinds = workRows(file);
  const preview = previewDraft(draft, { rules: file.rules, rows: kinds, models, names });
  const works = [{ value: ANY_WORK, label: "Any work" }, ...kinds.map((kind) => ({ value: kind.work, label: workName(kind.work) }))];
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
              <TargetPicker label="The model the rule is about" onPick={(picked) => set({ target: picked })} vendors={false}>
                <SettingsSelectTrigger label="The model the rule is about" placeholder={!target}>
                  {target ? names.target(target) : "Choose a model"}
                </SettingsSelectTrigger>
              </TargetPicker>
            </>
          )}
          for
          <SettingsSelect
            align="start"
            label="The work the rule is for"
            value={draft.work ?? ANY_WORK}
            options={works}
            onChange={(work) => set({ work: work === ANY_WORK ? null : work })}
          />
        </div>
        <Button variant="ghost" size="sm" onClick={onCancel}>Cancel</Button>
        <Button size="sm" disabled={(!capping && !target) || add.busy} onClick={submit}>
          Add rule
        </Button>
      </div>
      {preview && (
        <Note tone={preview.tone} icon={preview.tone === "info" ? <Info aria-hidden /> : <Warning aria-hidden />}>
          {preview.text}
          {preview.works?.map((work) => (
            <Button key={work} variant="link-blue" size="inline" className="ms-1.5 inline-flex" onClick={() => onOpenKind(work)}>
              Open {workName(work)}
            </Button>
          ))}
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
