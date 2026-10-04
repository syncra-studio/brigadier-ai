import { ChevronDown, ChevronRight, Reload } from "@openai/apps-sdk-ui/components/Icon";
import { useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import {
  SettingsButton,
  SettingsCard,
  SettingsRow,
  SettingsSection,
} from "@/app/settings/parts";
import { openUrl } from "@/ipc/client";
import type { ModelGroup } from "@/components/assistant-ui/elements/model-selector";
import { Badge } from "@/components/ui/badge";
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
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import type {
  Learned,
  MergedModel,
  ModelInfo,
  OverrideRule,
  ProviderKind,
  RatingProvenance,
  RegistryInfo,
  TaskCategory,
} from "@/ipc/generated";
import { formatAgo, formatDateTime, formatTokens } from "@/lib/format";
import {
  AREA_LABELS,
  CATEGORIES,
  CATEGORY_LABELS,
  formatDelta,
  joinWords,
  newRuleId,
  PLAIN_KIND_WORDS,
  PLAIN_TIERS,
  ruleSentence,
  TIER_LABELS,
} from "@/lib/routing";
import { cn } from "@/lib/utils";
import { blocksModel, isWorkerRule, mayWork } from "@/state/providers";
import { addOverride } from "@/state/routing";
import { useApp } from "@/state/store";
import { toast } from "@/state/toasts";
import { checkRegistry, setUsageProject } from "@/state/usage";

/** Categories a model is scored for, as the grid's short heads. */
const CATEGORY_HEADS: Record<TaskCategory, string> = {
  scout: "Scout",
  research: "Research",
  implement: "Implement",
  review: "Review",
  merge: "Merge",
  verify: "Verify",
  chat: "Chat",
  orchestrate: "Orchestrate",
};

/** A kind of work counts among a model's best within this much of its top score. */
const BEST_WITHIN = 0.5;
/** "Best at" names at most this many kinds of work. */
const BEST_SHOWN = 3;

function statusHint(model: MergedModel): string {
  if (model.excluded) return "Brigadier never routes work to this model.";
  if (model.trial) {
    return `An unrated model trying out low-risk work (scouting, research, verification) until ${model.trial.needed} outcomes place it.`;
  }
  switch (model.status) {
    case "curated":
      return "Scored by the curated registry.";
    case "inherited":
      return `A newer version of a registry model (${model.registryKey ?? "its family"}): it inherits its scores until outcomes say otherwise.`;
    case "researched":
      return "Not in the registry yet: scored from a research run's findings.";
    case "unknown":
      return "Not in the registry and not researched yet.";
  }
}

export const learnedKey = (provider: ProviderKind, model: string, category: TaskCategory) =>
  `${provider}\u0000${model}\u0000${category}`;

/**
 * "Best for writing code, reviewing changes and resolving conflicts": the kinds of work a
 * model scores highest for, in plain words. `null` for a model without scores.
 */
export function bestFor(model: MergedModel): string | null {
  const scored = CATEGORIES.flatMap((category) => {
    const strength = model.strengths[category];
    return strength === null || strength === undefined ? [] : [{ category, strength }];
  }).toSorted((a, b) => b.strength - a.strength);
  const top = scored[0]?.strength;
  if (top === undefined) return null;
  const best = scored.filter((entry) => entry.strength >= top - BEST_WITHIN).slice(0, BEST_SHOWN);
  return `Best for ${joinWords(best.map((entry) => PLAIN_KIND_WORDS[entry.category]))}`;
}

/**
 * A model in a few plain words: "Most capable · Best for writing code…" when Brigadier has
 * rated it, else the agent's own description of it.
 */
export function modelSummary(info: ModelInfo, merged: MergedModel | undefined): string {
  if (!merged || merged.tier === "unrated") return info.description;
  return [PLAIN_TIERS[merged.tier], bestFor(merged)].filter(Boolean).join(" · ");
}

/** Where a model's rating comes from, when not the curated registry: a badge's word and its tip. */
const PROVENANCE: Record<Exclude<RatingProvenance, "curated">, { label: string; hint: string }> = {
  researchedOverlay: {
    label: "Researched",
    hint: "Rated by the latest rankings refresh, which researched it on the web.",
  },
  researchNote: {
    label: "Estimated",
    hint: "Not in the curated registry: rated from a research run's notes.",
  },
  unrated: {
    label: "Unrated",
    hint: "Not in the curated registry and not researched yet.",
  },
};

/** A small badge saying where a model's rating comes from; nothing for a curated rating. */
export function ProvenanceBadge({ provenance }: { provenance: RatingProvenance }) {
  if (provenance === "curated") return null;
  const { label, hint } = PROVENANCE[provenance];
  return (
    <TooltipProvider>
      <Tooltip>
        <TooltipTrigger asChild>
          {/* Focusable, so the tip shows from the keyboard too. */}
          <Badge variant="outline" tabIndex={0} aria-label={`${label}: ${hint}`}>
            {label}
          </Badge>
        </TooltipTrigger>
        <TooltipContent>{hint}</TooltipContent>
      </Tooltip>
    </TooltipProvider>
  );
}

/** A source's address as a link, opened in the browser. */
export function SourceLink({ url }: { url: string }) {
  return (
    <button
      type="button"
      className="text-link max-w-full truncate text-start hover:underline"
      title={url}
      onClick={() =>
        openUrl(url).catch((cause: unknown) =>
          toast(`Couldn't open ${url}: ${cause instanceof Error ? cause.message : String(cause)}`, {
            tone: "error",
          }),
        )
      }
    >
      {url}
    </button>
  );
}

/** The registry in use, in a sentence, with "Check for updates". */
export function RegistryCard({ registry, now }: { registry: RegistryInfo; now: number }) {
  const check = useAction();
  const source =
    registry.source === "bundled"
      ? "bundled with this version of Brigadier"
      : `downloaded ${registry.fetchedAtMs !== null ? formatDateTime(registry.fetchedAtMs) : "from the repository"}`;
  const checked =
    registry.checkedAtMs !== null ? `checked ${formatAgo(registry.checkedAtMs, now)}` : "not checked yet";
  return (
    <SettingsSection>
      <SettingsCard>
        <SettingsRow
          label="Model registry"
          description={
            <>
              Revision {registry.revision} ({registry.updated}), {source} · {registry.models}{" "}
              models · {checked}
              {registry.error && (
                <span role="alert" className="text-warning block">
                  The last check didn't update it: {registry.error}
                </span>
              )}
            </>
          }
          error={check.error}
        >
          <SettingsButton disabled={check.busy} onClick={() => check.run(checkRegistry)}>
            <Reload className={check.busy ? "animate-spin motion-reduce:animate-none" : undefined} />
            Check for updates
          </SettingsButton>
        </SettingsRow>
      </SettingsCard>
    </SettingsSection>
  );
}

/** "All projects" or one project, for the learned adjustments. */
export function ProjectPicker({ projectId }: { projectId: string | null }) {
  const projects = useApp((s) => s.projects);
  const list = Object.values(projects).toSorted((a, b) => a.name.localeCompare(b.name));
  const label = projectId ? (projects[projectId]?.name ?? "Project") : "All projects";
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button size="xs" variant="outline" aria-label={`Learned in: ${label}`}>
          <span className="max-w-2xs truncate">{label}</span>
          <ChevronDown />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <DropdownMenuRadioGroup
          value={projectId ?? ""}
          onValueChange={(value) => setUsageProject(value === "" ? null : value)}
        >
          <DropdownMenuRadioItem value="">All projects</DropdownMenuRadioItem>
          {list.length > 0 && <DropdownMenuSeparator />}
          {list.map((project) => (
            <DropdownMenuRadioItem key={project.id} value={project.id}>
              {project.name}
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function learnedHint(entry: Learned): string {
  const parts = [
    `${entry.samples} outcomes`,
    `${Math.round(entry.successRate * 100)}% succeeded`,
    entry.reviewPassRate !== null && `${Math.round(entry.reviewPassRate * 100)}% passed review first time`,
    entry.avgRework !== null && `${entry.avgRework.toFixed(1)} rework rounds on average`,
    entry.verificationPassRate !== null &&
      `${Math.round(entry.verificationPassRate * 100)}% passed verification`,
    entry.medianDurationMs !== null && `median ${Math.round(entry.medianDurationMs / 60_000)} min`,
    entry.medianTokens !== null && `median ${formatTokens(entry.medianTokens)} tokens`,
  ].filter((part): part is string => typeof part === "string");
  return `${parts.join(" · ")}: ${formatDelta(entry.adjustment)} to its score here.`;
}

/**
 * One model under Advanced: its name, how capable it is and what it is best for, and the
 * user's rules about it. Its details show its score for every kind of work (with what
 * outcomes added or took away), its facts and "Don't use for…".
 */
export function ModelRow({
  model,
  learned,
  projectId,
  groups,
}: {
  model: MergedModel;
  learned: ReadonlyMap<string, Learned>;
  projectId: string | null;
  groups: readonly ModelGroup[];
}) {
  const [open, setOpen] = useState(false);
  const rules = useApp((s) => s.settings.routingOverrides);
  const projects = useApp((s) => s.projects);
  const ref = { provider: model.provider, id: model.id };
  const works = !model.excluded && mayWork(rules, ref);
  // The Routing switch's own rules show as the switch, not as sentences here.
  const own = rules.filter(
    (rule) =>
      !blocksModel(rule, ref) &&
      !isWorkerRule(rule, ref) &&
      rule.target.provider === model.provider &&
      ((rule.target.type === "model" && rule.target.id === model.id) ||
        (rule.target.type === "family" && rule.target.family === model.family)),
  );
  const summary = model.excluded
    ? "Brigadier never gives work to Fable models."
    : [
        PLAIN_TIERS[model.tier],
        bestFor(model),
        !works && "gets no work",
        works && model.trial && "new: tried on small tasks first",
      ]
        .filter(Boolean)
        .join(" · ");
  return (
    <div data-slot="usage-model" className="@container flex flex-col gap-2 px-4 py-3">
      <div className="flex min-w-0 flex-col gap-0.5">
        <span className="flex items-center gap-2">
          <span className={cn("text-label font-medium", !works && "text-muted-foreground")}>
            {model.displayName}
          </span>
          <ProvenanceBadge provenance={model.ratingProvenance} />
        </span>
        <span className="text-foreground/65 text-xs">{summary}</span>
        {own.length > 0 && (
          <span className="text-muted-foreground text-xs">
            {own.map((rule) => ruleSentence(rule, groups, projects)).join(". ")}.
          </span>
        )}
        <button
          type="button"
          aria-expanded={open}
          onClick={() => setOpen(!open)}
          className="text-muted-foreground hover:text-foreground flex w-fit items-center gap-1 pt-0.5 text-xs"
        >
          <ChevronRight
            aria-hidden
            className={cn(
              "size-icon-xs transition-transform motion-reduce:transition-none",
              open && "rotate-90",
            )}
          />
          Details
        </button>
      </div>
      {open && (
        <div className="flex flex-col gap-3 pt-1 ps-4">
          <dl className="grid grid-cols-4 gap-x-3 gap-y-2 @md:grid-cols-8">
            {CATEGORIES.map((category) => {
              const strength = model.strengths[category];
              const entry = learned.get(learnedKey(model.provider, model.id, category));
              return (
                <div key={category} className="flex min-w-0 flex-col">
                  <dt className="text-muted-foreground truncate text-2xs">
                    {CATEGORY_HEADS[category]}
                  </dt>
                  <dd className="flex items-baseline gap-1 text-xs tabular-nums">
                    <span>{strength ?? "–"}</span>
                    {entry && entry.adjustment !== 0 && (
                      <span className="text-muted-foreground text-2xs" title={learnedHint(entry)}>
                        {formatDelta(entry.adjustment)}
                      </span>
                    )}
                  </dd>
                </div>
              );
            })}
          </dl>
          <p className="text-muted-foreground text-xs">
            Scores run 0–10 per kind of work. {statusHint(model)}{" "}
            <span className="font-mono">{model.id}</span>
          </p>
          <ModelDetails model={model} />
          {!model.excluded && <NeverMenu model={model} projectId={projectId} groups={groups} />}
        </div>
      )}
    </div>
  );
}

function ModelDetails({ model }: { model: MergedModel }) {
  const areas = Object.entries(model.areaStrengths).filter(([, value]) => value !== 0);
  const facts = [
    model.contextWindow !== null && `${formatTokens(model.contextWindow)} context`,
    model.knowledgeCutoff && `knowledge to ${model.knowledgeCutoff}`,
    model.efforts.length > 0 && `efforts ${model.efforts.join(", ")}`,
    `input ${model.modalities.input.join(", ")}`,
    model.modalities.tools.length > 0 &&
      model.modalities.tools.map((tool) => (tool === "webSearch" ? "web search" : "image generation")).join(", "),
    model.registryKey && `registry: ${model.registryKey}`,
    model.family && `family: ${model.family}`,
  ].filter((fact): fact is string => typeof fact === "string" && fact.length > 0);
  const research = model.research;
  return (
    <div className="flex flex-col gap-2 text-xs">
      <p className="text-muted-foreground">{facts.join(" · ")}</p>
      {areas.length > 0 && (
        <p className="text-muted-foreground">
          By area:{" "}
          {areas
            .map(([area, value]) => `${AREA_LABELS[area as keyof typeof AREA_LABELS]} ${formatDelta(value ?? 0)}`)
            .join(", ")}
        </p>
      )}
      {research && (
        <div className="flex flex-col gap-1">
          <p>
            <span className="text-muted-foreground">
              Researched {formatDateTime(research.atMs)} · estimated {TIER_LABELS[research.tier]}:
            </span>{" "}
            {research.summary}
          </p>
          {research.sources.length > 0 && (
            <ul className="flex flex-col gap-0.5">
              {research.sources.map((source) => (
                <li key={source} className="min-w-0">
                  <SourceLink url={source} />
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </div>
  );
}

/** "Never use for…": adds a rule keeping this model from one kind of work, or any. */
function NeverMenu({
  model,
  projectId,
  groups,
}: {
  model: MergedModel;
  projectId: string | null;
  groups: readonly ModelGroup[];
}) {
  const projects = useApp((s) => s.projects);
  const [scope, setScope] = useState<string>(projectId ?? "");
  const add = (categories: TaskCategory[]) => {
    const rule: OverrideRule = {
      id: newRuleId(),
      effect: "never",
      target: { type: "model", provider: model.provider, id: model.id },
      categories,
      areas: [],
      projectId: scope === "" ? null : scope,
      createdAtMs: Date.now(),
    };
    const sentence = ruleSentence(rule, groups, projects);
    addOverride(rule)
      .then(() => toast(`Added: ${sentence}.`))
      .catch((cause: unknown) =>
        toast(`Couldn't add the rule: ${cause instanceof Error ? cause.message : String(cause)}`, {
          tone: "error",
        }),
      );
  };
  const projectName = projectId ? projects[projectId]?.name : undefined;
  return (
    <DropdownMenu onOpenChange={(open) => open && setScope(projectId ?? "")}>
      <DropdownMenuTrigger asChild>
        <Button size="xs" variant="outline" className="w-fit">
          Don't use for…
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-2xs">
        <DropdownMenuLabel>Never use {model.displayName} for</DropdownMenuLabel>
        <DropdownMenuItem onSelect={() => add([])}>Anything</DropdownMenuItem>
        {CATEGORIES.map((category) => (
          <DropdownMenuItem key={category} onSelect={() => add([category])}>
            {CATEGORY_LABELS[category].charAt(0).toUpperCase()}
            {CATEGORY_LABELS[category].slice(1)}
          </DropdownMenuItem>
        ))}
        {projectId && projectName && (
          <>
            <DropdownMenuSeparator />
            <DropdownMenuRadioGroup value={scope} onValueChange={setScope}>
              <DropdownMenuRadioItem value={projectId} onSelect={(event) => event.preventDefault()}>
                In {projectName}
              </DropdownMenuRadioItem>
              <DropdownMenuRadioItem value="" onSelect={(event) => event.preventDefault()}>
                Everywhere
              </DropdownMenuRadioItem>
            </DropdownMenuRadioGroup>
          </>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
