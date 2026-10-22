import { useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { BanIcon, Badge, Note, PreferIcon } from "@/app/settings/routing/parts";
import { TargetItems } from "@/app/settings/routing/pickers";
import {
  type Draft,
  type DraftEffect,
  draftKindsName,
  draftRule,
  type Names,
  previewDraft,
  ruleLine,
  ruleTitle,
  toggleKind,
} from "@/app/settings/routing/ruleWords";
import { ROUTED_KINDS, shortModelName, shortPlaceName } from "@/app/settings/routing/words";
import { SettingsSection } from "@/app/settings/parts";
import { type ModelGroup, SettingsCard } from "@/components/shared";
import { Button, SettingsSelect, SettingsSelectTrigger } from "@/components/controls";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/overlays";
import { DotsHorizontal, Info, Plus, Warning } from "@/components/icons";
import type { OverrideRule, OverrideTarget, RoutePreview } from "@/ipc/generated";
import { newRuleId, PLAIN_KIND_NAMES, ruleSentence } from "@/lib/routing";
import { isNewModelRule } from "@/state/providers";
import { addOverride, removeOverride } from "@/state/routing";
import { useApp } from "@/state/store";
import { toast } from "@/state/toasts";
import { useUsage } from "@/state/usage";

export const RULES = {
  label: "Rules",
  description: "Keep a model away from some work, or prefer it for some work",
} as const;

const EFFECTS: { value: DraftEffect; label: string }[] = [
  { value: "never", label: "Never" },
  { value: "prefer", label: "Prefer" },
];


/** The names the rules' words need, from the model list and the projects. */
function useNames(groups: readonly ModelGroup[]): Names {
  const projects = useApp((s) => s.projects);
  return {
    target: (target) => shortPlaceName(target, groups),
    model: (provider, model) => shortModelName(groups, provider, model),
    project: (id) => projects[id]?.name ?? null,
  };
}

/**
 * Rules: each as a sentence with where it applies, removed from its menu. Add rule opens a
 * sentence to complete at the top of the card, which says what it would change before it is
 * added; an added rule is marked New and a toast offers Undo.
 */
export function Rules({ routes, groups }: { routes: RoutePreview[] | null; groups: readonly ModelGroup[] }) {
  const rules = useApp((s) => s.settings.routingOverrides);
  const [adding, setAdding] = useState(false);
  const [fresh, setFresh] = useState<string | null>(null);
  const remove = useAction();
  const names = useNames(groups);
  const projects = useApp((s) => s.projects);
  const added = rules.find((rule) => rule.id === fresh);
  // The rule just added leads the list while it is new.
  const shown = added ? [added, ...rules.filter((rule) => rule !== added)] : rules;
  const onAdded = (rule: OverrideRule) => {
    setAdding(false);
    setFresh(rule.id);
    toast("Rule added", { actions: [{ label: "Undo", run: () => void removeOverride(rule.id) }] });
  };
  return (
    <SettingsSection
      title={RULES.label}
      description={RULES.description}
      actions={
        <Button variant="soft" size="sm" data-slot="add-rule" disabled={adding} onClick={() => setAdding(true)}>
          <Plus aria-hidden />
          Add rule
        </Button>
      }
    >
      {(shown.length > 0 || adding) && (
        <SettingsCard inset data-slot="routing-rules">
          {adding && <DraftRow routes={routes} names={names} onCancel={() => setAdding(false)} onAdded={onAdded} />}
          {shown.map((rule) => (
            <div
              key={rule.id}
              data-slot="routing-rule"
              // Where a rule's link from Providers lands: its sentence.
              data-setting={ruleSentence(rule, groups, projects)}
              className="flex items-center gap-3.5 py-3.5">
              {rule.effect === "never" ? <BanIcon /> : <PreferIcon />}
              <span className="flex min-w-0 flex-1 flex-col gap-1">
                <span className="flex min-w-0 items-center gap-2">
                  <span className="text-label text-foreground truncate font-medium">{ruleTitle(rule, names)}</span>
                  {rule.id === fresh && <Badge tone="link">New</Badge>}
                </span>
                <span className="text-xs text-foreground/65 truncate">
                  {ruleLine(rule, names, isNewModelRule(rule))}
                </span>
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
                  <DropdownMenuItem disabled={remove.busy} onSelect={() => remove.run(() => removeOverride(rule.id))}>
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

/** A new rule as a sentence to complete: "[Never] use [a model] for [Any work]", then what it would change. */
function DraftRow({
  routes,
  names,
  onCancel,
  onAdded,
}: {
  routes: RoutePreview[] | null;
  names: Names;
  onCancel: () => void;
  onAdded: (rule: OverrideRule) => void;
}) {
  const rules = useApp((s) => s.settings.routingOverrides);
  const rankings = useApp((s) => s.settings.routingRankings);
  const models = useUsage((s) => s.view?.models);
  const [draft, setDraft] = useState<Draft>({ effect: "never", target: null, kinds: [] });
  const add = useAction();
  const target = draft.target;
  const preview = previewDraft(draft, {
    rules,
    rankings,
    routes,
    names,
    family: (provider, model) => models?.find((found) => found.provider === provider && found.id === model)?.family ?? null,
  });
  const submit = () => {
    if (!target) return;
    const rule = draftRule({ ...draft, target }, newRuleId(), Date.now());
    add.run(async () => {
      await addOverride(rule);
      onAdded(rule);
    });
  };
  const set = (change: Partial<Draft>) => setDraft((current) => ({ ...current, ...change }));
  return (
    <div data-slot="routing-rule-form" className="flex flex-col gap-3 py-4">
      <div className="flex items-center gap-3.5">
        {draft.effect === "never" ? <BanIcon /> : <PreferIcon />}
        <div className="text-label text-foreground/65 flex min-w-0 flex-1 flex-wrap items-center gap-2">
          <SettingsSelect align="start" label="What the rule does" value={draft.effect} options={EFFECTS} onChange={(effect) => set({ effect })} />
          use
          <DropdownMenu modal={false}>
            <DropdownMenuTrigger asChild>
              <SettingsSelectTrigger label="The model the rule is about" placeholder={!target}>
                {target ? names.target(target) : "Choose a model"}
              </SettingsSelectTrigger>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="start" className="max-h-96 w-xs overflow-y-auto">
              <TargetItems onPick={(picked: OverrideTarget) => set({ target: picked })} />
            </DropdownMenuContent>
          </DropdownMenu>
          for
          <DropdownMenu modal={false}>
            <DropdownMenuTrigger asChild>
              <SettingsSelectTrigger label="The kinds of work the rule is about">{draftKindsName(draft.kinds)}</SettingsSelectTrigger>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="start" className="max-h-96 max-w-sm min-w-(--radix-dropdown-menu-trigger-width) overflow-y-auto">
              {/* Any work stands alone; kinds tick together, the menu staying open. */}
              <DropdownMenuCheckboxItem checked={draft.kinds.length === 0} onCheckedChange={() => set({ kinds: [] })}>
                Any work
              </DropdownMenuCheckboxItem>
              {ROUTED_KINDS.map((kind) => (
                <DropdownMenuCheckboxItem
                  key={kind}
                  checked={draft.kinds.includes(kind)}
                  onSelect={(event) => event.preventDefault()}
                  onCheckedChange={() => set({ kinds: toggleKind(draft.kinds, kind) })}
                >
                  {PLAIN_KIND_NAMES[kind]}
                </DropdownMenuCheckboxItem>
              ))}
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
        <Button variant="ghost" size="sm" onClick={onCancel}>Cancel</Button>
        <Button size="sm" disabled={!target || add.busy} onClick={submit}>
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
