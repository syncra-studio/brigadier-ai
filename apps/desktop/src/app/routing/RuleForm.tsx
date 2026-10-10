import { ChevronDown } from "@openai/apps-sdk-ui/components/Icon";
import { useEffect, useMemo, useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { ErrorLine, Field } from "@/app/dialogs/fields";
import type { ModelGroup } from "@/components/assistant-ui/elements/model-selector";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import type {
  Area,
  OverrideEffect,
  OverrideRule,
  OverrideTarget,
  TaskCategory,
} from "@/ipc/generated";
import {
  AREA_LABELS,
  AREAS,
  CATEGORIES,
  CATEGORY_LABELS,
  EFFECT_LABELS,
  isFable,
  newRuleId,
  noWorkRule,
  PROVIDERS,
  ruleSentence,
  targetName,
  VENDOR_LABELS,
} from "@/lib/routing";
import { useAvailableModelGroups } from "@/lib/setup";
import { providerOn } from "@/state/providers";
import { addOverride } from "@/state/routing";
import { cn } from "@/lib/utils";
import { useApp } from "@/state/store";
import { loadUsage, useUsage } from "@/state/usage";

/** What a rule does: "Don't give work" is a `never` for every worker kind of work, everywhere. */
type Kind = OverrideEffect | "noWork";

const KINDS: readonly Kind[] = ["noWork", "never", "prefer", "only"];

const KIND_LABELS: Record<Kind, string> = { noWork: "Don't give work", ...EFFECT_LABELS };

const KIND_HINTS: Record<Kind, string> = {
  noWork: "Brigadier gives it no tasks. You can still pick it for a chat or session.",
  never: "Routing never picks it.",
  prefer: "Routing picks it whenever it's available (where routing scores).",
  only: "Routing picks only it; work waits when it's at a limit.",
};

/** Each provider's family words, from the merged model list (read on opening). */
export function useFamilies(): { families: Map<string, string[]>; error: string | null } {
  const models = useUsage((s) => s.view?.models);
  const error = useUsage((s) => s.error);
  useEffect(() => {
    if (!useUsage.getState().view) void loadUsage();
  }, []);
  const families = useMemo(() => {
    const byProvider = new Map<string, string[]>();
    for (const model of models ?? []) {
      if (!model.family || model.excluded) continue;
      const list = byProvider.get(model.provider) ?? [];
      if (!list.includes(model.family)) list.push(model.family);
      byProvider.set(model.provider, list);
    }
    return byProvider;
  }, [models]);
  return { families, error: models ? null : error };
}

/** A new rule: its effect, model, kinds of work, areas and scope, added as one. */
export function RuleForm({
  groups,
  onDone,
  className,
}: {
  groups: readonly ModelGroup[];
  onDone: () => void;
  className?: string;
}) {
  const projects = useApp((s) => s.projects);
  const [kind, setKind] = useState<Kind>("noWork");
  const [target, setTarget] = useState<OverrideTarget | null>(null);
  const [categories, setCategories] = useState<TaskCategory[]>([]);
  const [areas, setAreas] = useState<Area[]>([]);
  const [projectId, setProjectId] = useState<string | null>(null);
  const add = useAction();

  const noWork = kind === "noWork";
  const rule: OverrideRule | null =
    target &&
    (noWork
      ? noWorkRule(target, "", 0)
      : { id: "", effect: kind, target, categories, areas, projectId, createdAtMs: 0 });
  const submit = () => {
    if (!rule) return;
    add.run(async () => {
      await addOverride({ ...rule, id: newRuleId(), createdAtMs: Date.now() });
      onDone();
    });
  };

  return (
    <div
      data-slot="routing-rule-form"
      className={cn("bg-card rounded-surface grid gap-3 border p-3", className)}
    >
      <Field label="Effect" hint={KIND_HINTS[kind]}>
        <ToggleGroup
          type="single"
          size="sm"
          variant="outline"
          spacing="tight"
          aria-label="Effect"
          value={kind}
          onValueChange={(value) => value && setKind(value as Kind)}
        >
          {KINDS.map((entry) => (
            <ToggleGroupItem key={entry} value={entry} className="text-xs">
              {KIND_LABELS[entry]}
            </ToggleGroupItem>
          ))}
        </ToggleGroup>
      </Field>
      <Field label="Model">
        <TargetPicker groups={groups} value={target} onChange={setTarget} />
      </Field>
      {!noWork && (
        <>
          <Field label="For" hint="None picked: any kind of work.">
            <ToggleGroup
              type="multiple"
              size="sm"
              variant="outline"
              spacing="tight"
              aria-label="Kinds of work"
              className="flex-wrap"
              value={categories}
              onValueChange={(value) => setCategories(value as TaskCategory[])}
            >
              {CATEGORIES.map((category) => (
                <ToggleGroupItem key={category} value={category} className="text-xs">
                  {CATEGORY_LABELS[category]}
                </ToggleGroupItem>
              ))}
            </ToggleGroup>
          </Field>
          <Field label="Touching" hint="None picked: any part of the codebase.">
            <ToggleGroup
              type="multiple"
              size="sm"
              variant="outline"
              spacing="tight"
              aria-label="Areas"
              className="flex-wrap"
              value={areas}
              onValueChange={(value) => setAreas(value as Area[])}
            >
              {AREAS.map((area) => (
                <ToggleGroupItem key={area} value={area} className="text-xs">
                  {AREA_LABELS[area]}
                </ToggleGroupItem>
              ))}
            </ToggleGroup>
          </Field>
          <Field label="Where">
            <ScopePicker value={projectId} onChange={setProjectId} />
          </Field>
        </>
      )}
      {rule && (
        <p className="text-sm" aria-live="polite">
          {ruleSentence(rule, groups, projects)}
        </p>
      )}
      <ErrorLine error={add.error} />
      <div className="flex justify-end gap-2">
        <Button type="button" size="sm" variant="ghost" onClick={onDone}>
          Cancel
        </Button>
        <Button type="button" size="sm" disabled={!rule || add.busy} onClick={submit}>
          Add rule
        </Button>
      </div>
    </div>
  );
}

/** A vendor, one of its families, or one of its models from the live lists. */
function TargetPicker({
  groups,
  value,
  onChange,
}: {
  groups: readonly ModelGroup[];
  value: OverrideTarget | null;
  onChange: (target: OverrideTarget) => void;
}) {
  const { families, error } = useFamilies();
  // Only what the user made available is offered; names still come from every list.
  const available = useAvailableModelGroups();
  const availability = useApp((s) => s.settings);
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button type="button" size="sm" variant="outline" className="max-w-full justify-between">
          <span className="truncate">{value ? targetName(value, groups) : "Pick a model…"}</span>
          <ChevronDown />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-xs">
        {PROVIDERS.filter((provider) => providerOn(availability, provider)).map((provider, index) => {
          const group = available.find((entry) => entry.provider === provider);
          // Routing never uses a Fable model, so no rule is about one.
          const models = (group?.models ?? []).filter((model) => !model.legacy && !isFable(model));
          const words = families.get(provider) ?? [];
          return (
            <div key={provider}>
              {index > 0 && <DropdownMenuSeparator />}
              <DropdownMenuLabel>{VENDOR_LABELS[provider]}</DropdownMenuLabel>
              <DropdownMenuItem onSelect={() => onChange({ type: "vendor", provider })}>
                Any {VENDOR_LABELS[provider]} model
              </DropdownMenuItem>
              {words.map((family) => (
                <DropdownMenuItem
                  key={family}
                  onSelect={() => onChange({ type: "family", provider, family })}
                >
                  {VENDOR_LABELS[provider]} {family} models
                </DropdownMenuItem>
              ))}
              {models.map((model) => (
                <DropdownMenuItem
                  key={model.id}
                  onSelect={() => onChange({ type: "model", provider, id: model.id })}
                >
                  <span className="truncate">{model.displayName}</span>
                  <span className="text-muted-foreground ms-auto truncate font-mono text-2xs">
                    {model.id}
                  </span>
                </DropdownMenuItem>
              ))}
            </div>
          );
        })}
        {error && (
          <p className="text-muted-foreground px-2 py-1.5 text-xs">
            Families appear once the model list can be read ({error}).
          </p>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/** Everywhere, or one project. */
function ScopePicker({
  value,
  onChange,
}: {
  value: string | null;
  onChange: (projectId: string | null) => void;
}) {
  const projects = useApp((s) => s.projects);
  const list = Object.values(projects).toSorted((a, b) => a.name.localeCompare(b.name));
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button type="button" size="sm" variant="outline" className="w-fit">
          <span className="max-w-2xs truncate">
            {value ? `In ${projects[value]?.name ?? "a removed project"}` : "Everywhere"}
          </span>
          <ChevronDown />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start">
        <DropdownMenuRadioGroup
          value={value ?? ""}
          onValueChange={(next) => onChange(next === "" ? null : next)}
        >
          <DropdownMenuRadioItem value="">Everywhere</DropdownMenuRadioItem>
          {list.length > 0 && <DropdownMenuSeparator />}
          {list.map((project) => (
            <DropdownMenuRadioItem key={project.id} value={project.id}>
              In {project.name}
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
