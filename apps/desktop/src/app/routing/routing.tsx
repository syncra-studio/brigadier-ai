import { ChevronDown } from "@openai/apps-sdk-ui/components/Icon";
import { useState } from "react";

import { CategoryCard } from "@/app/routing/CategoryCard";
import { KindRow } from "@/app/routing/KindPicker";
import { RULES_ROW, RulesSection } from "@/app/routing/RulesSection";
import {
  type Choice,
  SettingsButton,
  SettingsCard,
  SettingsSection,
  SettingsSelect,
} from "@/app/settings/parts";
import type { ModelGroup } from "@/components/assistant-ui/elements/model-selector";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import type { Area } from "@/ipc/generated";
import { AREA_LABELS, AREAS, RANKED_CATEGORIES } from "@/lib/routing";
import { resetToAutomatic, useRoutePreview } from "@/state/routing";
import { useApp } from "@/state/store";

/** Routing's rows on the Providers page, for Settings search; the page renders this copy. */
export const ROUTING_ROWS = {
  simple: {
    label: "Who does what",
    description:
      "Which model does each kind of work Brigadier hands out. Leave it on Automatic, or pick a model. The chat's own model is picked in the composer.",
  },
  advanced: {
    label: "Advanced",
    description:
      "The full order and why, backup models, different settings per project or area, and rules like “never use this model for reviews”.",
  },
  kinds: {
    label: "Kinds of work in detail",
    description:
      "Open one to see every model in the order routing would try it, and why, or to set an order of your own.",
  },
  modes: {
    label: "Automatic or Manual",
    description: "Let routing rank the models for a kind of work, or try your own list top-down.",
  },
  areas: {
    label: "Area overrides",
    description: "A different list or rule for work that touches frontend, backend, infra, docs or tests.",
  },
  reset: {
    label: "Reset to automatic",
    description: "Every kind of work goes back to Automatic; your lists are kept.",
  },
  rules: RULES_ROW,
} as const;

/**
 * Who does what: one row per kind of work in plain words, each with one choice: Automatic
 * (what Brigadier would pick now shows) or a model the user picks.
 */
export function RoutingKinds({ groups }: { groups: readonly ModelGroup[] }) {
  const { routes, error } = useRoutePreview(null, NO_AREAS);
  return (
    <SettingsSection title={ROUTING_ROWS.simple.label} description={ROUTING_ROWS.simple.description}>
      {error && (
        <p role="alert" className="text-destructive text-xs">
          {error}
        </p>
      )}
      <SettingsCard>
        {RANKED_CATEGORIES.map((category) => (
          <KindRow
            key={category}
            category={category}
            route={routes?.find((route) => route.category === category) ?? null}
            groups={groups}
          />
        ))}
      </SettingsCard>
    </SettingsSection>
  );
}

const NO_AREAS: Area[] = [];

/**
 * Routing in full: per kind of work, whether Brigadier ranks the models (Automatic, with the
 * live order and why) or tries the user's list (Manual), everywhere or in one project, with
 * area overrides; then the user's rules.
 */
export function AdvancedRouting({ groups }: { groups: readonly ModelGroup[] }) {
  const projects = useApp((s) => s.projects);
  const rankings = useApp((s) => s.settings.routingRankings);
  const [scope, setScope] = useState<string | null>(null);
  const [areas, setAreas] = useState<Area[]>([]);
  const [resetting, setResetting] = useState(false);
  const { routes, error } = useRoutePreview(scope, areas);

  const scopes: Choice<string>[] = [
    { value: "", label: "Everywhere" },
    ...Object.values(projects)
      .toSorted((a, b) => a.name.localeCompare(b.name))
      .map((project) => ({ value: project.id, label: `In ${project.name}` })),
  ];
  const manualHere = rankings.some((ranking) => ranking.projectId === scope && ranking.manual);

  return (
    <>
      <SettingsSection
        title={ROUTING_ROWS.kinds.label}
        description={ROUTING_ROWS.kinds.description}
        actions={
          <>
            <SettingsSelect
              label="Where these settings apply"
              value={scope ?? ""}
              options={scopes}
              onChange={(value) => setScope(value === "" ? null : value)}
            />
            {manualHere && (
              <SettingsButton onClick={() => setResetting(true)}>
                {ROUTING_ROWS.reset.label}
              </SettingsButton>
            )}
          </>
        }
      >
        <div className="flex justify-end">
          <AreaPicker areas={areas} onChange={setAreas} />
        </div>
        {error && (
          <p role="alert" className="text-destructive text-xs">
            {error}
          </p>
        )}
        <SettingsCard>
          {RANKED_CATEGORIES.map((category) => (
            <CategoryCard
              key={category}
              category={category}
              route={routes?.find((route) => route.category === category) ?? null}
              scope={scope}
              groups={groups}
            />
          ))}
        </SettingsCard>
      </SettingsSection>
      <RulesSection groups={groups} />
      <p className="text-muted-foreground text-xs">
        Brigadier's own rules hold whatever you choose: no Fable models, effort at most high, a
        model at its limit waits until it resets, and a change is reviewed by another vendor when
        one can.
      </p>
      <ResetDialog
        open={resetting}
        scope={scope ? (projects[scope]?.name ?? "this project") : null}
        onOpenChange={setResetting}
        onConfirm={() => resetToAutomatic(scope)}
      />
    </>
  );
}

/** The areas the preview asks about ("a task touching frontend"). */
function AreaPicker({ areas, onChange }: { areas: Area[]; onChange: (areas: Area[]) => void }) {
  const label =
    areas.length === 0
      ? "Any area"
      : areas.map((area) => AREA_LABELS[area]).join(", ").replace(/^./, (first) => first.toUpperCase());
  return (
    <DropdownMenu modal={false}>
      <DropdownMenuTrigger asChild>
        <Button type="button" size="xs" variant="ghost" aria-label={`Preview for tasks touching: ${label}`}>
          <span className="text-muted-foreground">Previewing</span>
          {label.charAt(0).toLowerCase()}
          {label.slice(1)}
          <ChevronDown />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <DropdownMenuLabel>Preview for tasks touching</DropdownMenuLabel>
        {AREAS.map((area) => (
          <DropdownMenuCheckboxItem
            key={area}
            checked={areas.includes(area)}
            onSelect={(event) => event.preventDefault()}
            onCheckedChange={(checked) =>
              onChange(checked ? [...areas, area] : areas.filter((other) => other !== area))
            }
          >
            {AREA_LABELS[area].charAt(0).toUpperCase()}
            {AREA_LABELS[area].slice(1)}
          </DropdownMenuCheckboxItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function ResetDialog({
  open,
  scope,
  onOpenChange,
  onConfirm,
}: {
  open: boolean;
  /** The project's name; `null`: everywhere. */
  scope: string | null;
  onOpenChange: (open: boolean) => void;
  onConfirm: () => Promise<void>;
}) {
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const confirm = async () => {
    setBusy(true);
    setError(null);
    try {
      await onConfirm();
      onOpenChange(false);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>Reset {scope ? `${scope}'s routing` : "routing"} to automatic?</DialogTitle>
          <DialogDescription>
            Every kind of work and area override {scope ? `in ${scope}` : "everywhere"} goes back to
            Automatic. Your lists are kept, so switching a row to Manual brings its list back; your
            rules stay as they are.
          </DialogDescription>
        </DialogHeader>
        {error && (
          <p role="alert" className="text-destructive text-xs">
            {error}
          </p>
        )}
        <DialogFooter>
          <Button type="button" variant="ghost" size="sm" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button type="button" size="sm" disabled={busy} onClick={() => void confirm()}>
            Reset to automatic
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
