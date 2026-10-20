import { type ComponentType, type SVGProps, useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { Badge, HAIRLINE_TOP, OpenChevron, PillChoice, PillTrigger } from "@/app/routing/parts";
import { TargetItems } from "@/app/routing/pickers";
import {
  decides,
  orderSummary,
  reviewers,
  reviewsWords,
  ROUTED_KINDS,
  rowRanking,
  shortModelName,
  shortPlaceName,
} from "@/app/routing/words";
import { SettingsSection } from "@/app/settings/parts";
import { effortLabel, type ModelGroup } from "@/components/assistant-ui/elements/model-selector";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  BookOpen,
  Chat,
  Check,
  CheckCircle,
  Code,
  CommitIcon,
  DotsHorizontal,
  Grid,
  GripIcon,
  Nodes,
  Plus,
  ProviderGlyph,
  PullRequestMerged,
  PullRequestOpen,
  Telescope,
} from "@/components/icons";
import { RowText, SettingsCard } from "@/components/ui/settings-card";
import { useDragReorder } from "@/hooks/use-drag-reorder";
import type { OverrideTarget, RankedEntry, Ranking, RoutePreview, TaskCategory } from "@/ipc/generated";
import { formatCountdown } from "@/lib/format";
import { DEFAULT_FLOORS, joinWords, PLAIN_KIND_NAMES, TIER_LABELS, tierBelow } from "@/lib/routing";
import { cn } from "@/lib/utils";
import { changeRanking, newRankingId } from "@/state/routing";
import { useApp } from "@/state/store";

/** Each kind of work's icon, as the design draws it. */
const KIND_ICONS: Record<TaskCategory, ComponentType<SVGProps<SVGSVGElement>>> = {
  scout: Telescope,
  research: BookOpen,
  implement: Code,
  review: PullRequestOpen,
  merge: PullRequestMerged,
  verify: CheckCircle,
  operate: Grid,
  chat: Chat,
  orchestrate: Nodes,
  gitText: CommitIcon,
};

/** The efforts a model of an order may ask for: routing never goes above high. */
const EFFORTS = ["low", "medium", "high"] as const;

export const WHO_DOES_WHAT = {
  label: "Who does what",
  description: "Automatic picks the strongest model that can run right now. Open a kind of work to set your own order.",
} as const;

/** A model of an order's key: its place moves while it is dragged, its target doesn't. */
export function placeKey(entry: RankedEntry): string {
  const target = entry.target;
  return `${target.type}:${target.provider}:${target.type === "family" ? target.family : target.type === "model" ? target.id : ""}`;
}

/** Auto, then the levels up to high the model takes (all three for a family or a vendor). */
function effortLevels(target: OverrideTarget, groups: readonly ModelGroup[]): readonly string[] {
  const model =
    target.type === "model"
      ? groups.find((group) => group.provider === target.provider)?.models.find((found) => found.id === target.id)
      : undefined;
  return model ? EFFORTS.filter((level) => model.efforts.includes(level)) : EFFORTS;
}

/**
 * Who does what: a row per kind of work with how it picks and what it runs on now; a row opens
 * in place to Automatic (the models it would try) or My order (the order you set).
 */
export function WhoDoesWhat({
  routes,
  groups,
  now,
  onSeeStrengths,
}: {
  routes: RoutePreview[] | null;
  groups: readonly ModelGroup[];
  now: number;
  /** Opens Model strength on a kind of work. */
  onSeeStrengths: (category: TaskCategory) => void;
}) {
  const [open, setOpen] = useState<TaskCategory | null>(null);
  return (
    <SettingsSection title={WHO_DOES_WHAT.label} description={WHO_DOES_WHAT.description}>
      <SettingsCard inset data-slot="who-does-what">
        {ROUTED_KINDS.map((category) => (
          <KindRow
            key={category}
            category={category}
            open={open === category}
            route={routes?.find((found) => found.category === category) ?? null}
            groups={groups}
            now={now}
            onToggle={() => setOpen(open === category ? null : category)}
            onSeeStrengths={() => onSeeStrengths(category)}
          />
        ))}
      </SettingsCard>
    </SettingsSection>
  );
}

function KindRow({
  category,
  open,
  route,
  groups,
  now,
  onToggle,
  onSeeStrengths,
}: {
  category: TaskCategory;
  open: boolean;
  route: RoutePreview | null;
  groups: readonly ModelGroup[];
  now: number;
  onToggle: () => void;
  onSeeStrengths: () => void;
}) {
  const rankings = useApp((s) => s.settings.routingRankings);
  const own = rowRanking(rankings, category, null);
  const deciding = decides(own) ? own : null;
  const Icon = KIND_ICONS[category];
  const line = deciding
    ? `Your order · ${orderSummary(deciding, groups)}`
    : category === "chat"
      ? "The chat's own model, then Automatic"
      : "Automatic";
  return (
    <div data-slot="routing-kind" data-open={open || undefined} className="flex flex-col">
      <button type="button" aria-expanded={open} onClick={onToggle} className="flex w-full items-center gap-3.5 py-3.5 text-start">
        <Icon aria-hidden className="text-foreground/65 size-4 shrink-0" />
        <RowText truncate title={PLAIN_KIND_NAMES[category]} line={line} />
        <NowPick category={category} route={route} groups={groups} now={now} />
        <OpenChevron open={open} />
      </button>
      {open && (
        <WhoPicks category={category} own={own} deciding={deciding} route={route} groups={groups} onSeeStrengths={onSeeStrengths} />
      )}
    </div>
  );
}

/** "Now ✻ Opus 5.5": what the kind runs on now; a chat's varies; a kind that waits says so. */
function NowPick({
  category,
  route,
  groups,
  now,
}: {
  category: TaskCategory;
  route: RoutePreview | null;
  groups: readonly ModelGroup[];
  now: number;
}) {
  // A chat's own model, or a review's for each vendor's change, varies.
  if (category === "chat" || reviewers(route)) {
    return <span className="text-xs text-muted-foreground shrink-0">Varies</span>;
  }
  if (!route) return null;
  const outcome = route.outcome;
  return (
    <span data-slot="now-pick" className="flex shrink-0 items-center gap-2">
      <span className="text-xs text-muted-foreground">Now</span>
      {outcome.type === "wait" ? (
        <span className="text-xs text-warning">
          Waits{outcome.resetsAtMs !== null && ` · ${formatCountdown(outcome.resetsAtMs, now)}`}
        </span>
      ) : (
        <span className="flex items-center gap-1.75">
          <ProviderGlyph provider={outcome.choice.provider} className="shrink-0 size-3.5" />
          <span className="text-xs text-foreground">
            {shortModelName(groups, outcome.choice.provider, outcome.choice.model ?? "")}
          </span>
        </span>
      )}
    </span>
  );
}

/** An opened kind: Who picks (Automatic or My order), then its models. */
function WhoPicks({
  category,
  own,
  deciding,
  route,
  groups,
  onSeeStrengths,
}: {
  category: TaskCategory;
  own: Ranking | null;
  deciding: Ranking | null;
  route: RoutePreview | null;
  groups: readonly ModelGroup[];
  onSeeStrengths: () => void;
}) {
  const action = useAction();
  const name = PLAIN_KIND_NAMES[category];
  // My order with nothing to start it from: an empty order, its Add model there to fill it.
  const [starting, setStarting] = useState(false);
  const save = (next: RankedEntry[]) => {
    setStarting(false);
    return action.run(() =>
      changeRanking(
        own?.id ?? "",
        // Removing the last model leaves the kind on Automatic.
        (current) => ({ ...current, entries: next, manual: next.length > 0 }),
        () => created(true, []),
      ),
    );
  };
  const created = (manual: boolean, entries: RankedEntry[]): Ranking => ({
    id: newRankingId(),
    category,
    areas: [],
    projectId: null,
    manual,
    entries,
    only: false,
    updatedAtMs: Date.now(),
  });
  // Automatic keeps the order to come back to; My order brings it back, else starts one with
  // the model picked now.
  const choose = (mode: "automatic" | "order") => {
    if (mode === "automatic") {
      setStarting(false);
      return action.run(() => changeRanking(own?.id ?? "", (current) => ({ ...current, manual: false }), () => created(false, [])));
    }
    if (own && own.entries.length > 0) return action.run(() => changeRanking(own.id, (current) => ({ ...current, manual: true })));
    const picked = route?.outcome.type === "chosen" ? route.outcome.choice : null;
    if (!picked) return setStarting(true);
    return save([{ target: { type: "model", provider: picked.provider, id: picked.model ?? "" }, effort: null }]);
  };
  const ordering = deciding !== null || starting;
  return (
    <div data-slot="who-picks" className="flex flex-col ps-7.5 pb-1">
      <div className={cn(HAIRLINE_TOP, "flex items-center gap-6 py-3.5")}>
        <RowText
          truncate
          quiet
          title="Who picks"
          line={ordering ? "Brigadier, or the order you set" : "The strongest model that can run right now"}
        />
        <PillChoice
          label={`How ${name} picks a model`}
          value={ordering ? "order" : "automatic"}
          options={[
            { value: "automatic", label: "Automatic" },
            { value: "order", label: "My order" },
          ]}
          disabled={action.busy}
          onChange={choose}
        />
      </div>
      {ordering ? (
        <OrderList name={name} entries={deciding?.entries ?? []} route={route} groups={groups} onSave={save} />
      ) : (
        <AutoPicks category={category} route={route} groups={groups} onSeeStrengths={onSeeStrengths} />
      )}
      {action.error && (
        <p role="alert" className="text-destructive text-xs pb-3">
          {action.error}
        </p>
      )}
    </div>
  );
}

/**
 * Automatic: the models that can take the work, in the order routing would try them, the one
 * picked now marked and the next named; then those skipped as below the tier the work needs.
 */
function AutoPicks({
  category,
  route,
  groups,
  onSeeStrengths,
}: {
  category: TaskCategory;
  route: RoutePreview | null;
  groups: readonly ModelGroup[];
  onSeeStrengths: () => void;
}) {
  const candidates = route?.candidates ?? [];
  const eligible = candidates.filter((candidate) => candidate.blocked === null);
  const floor = DEFAULT_FLOORS[category];
  const skipped = candidates.filter((candidate) => candidate.blocked !== null && tierBelow(candidate.tier, floor));
  const name = (provider: (typeof candidates)[number]["provider"], model: string) => shortModelName(groups, provider, model);
  // A review picked for each vendor's change marks whose changes each model reviews instead.
  const byAuthor = reviewers(route);
  const picked = byAuthor ? undefined : eligible.find((candidate) => candidate.chosen);
  return (
    <div data-slot="auto-picks" className={cn(HAIRLINE_TOP, "flex flex-col pt-1 pb-3")}>
      {eligible.map((candidate, index) => (
        <div key={`${candidate.provider}/${candidate.model}`} data-slot="auto-pick" className="flex h-10 items-center gap-3">
          <span className="text-xs text-muted-foreground w-2 shrink-0 font-mono">{index + 1}</span>
          <span className="flex min-w-0 flex-1 items-center gap-2">
            <ProviderGlyph provider={candidate.provider} className="shrink-0 size-3.5" />
            <span className="text-label text-foreground truncate">{name(candidate.provider, candidate.model)}</span>
          </span>
          {byAuthor ? (
            byAuthor.has(`${candidate.provider}/${candidate.model}`) && (
              <Badge tone="success">{reviewsWords(byAuthor.get(`${candidate.provider}/${candidate.model}`)!)}</Badge>
            )
          ) : candidate.chosen ? (
            <Badge tone="success">Picked now</Badge>
          ) : (
            picked &&
            index === 1 && (
              <span className="text-xs text-muted-foreground truncate">
                Next if {name(picked.provider, picked.model)} can't run
              </span>
            )
          )}
          <span className="text-xs text-foreground w-7.5 shrink-0 text-end font-mono font-medium">
            {candidate.score?.toFixed(1)}
          </span>
        </div>
      ))}
      {skipped.length > 0 && (
        <p className="text-xs text-muted-foreground pt-1">
          Skipped: {joinWords(skipped.map((candidate) => name(candidate.provider, candidate.model)))}{" "}
          {skipped.length === 1 ? "is" : "are"} below the {TIER_LABELS[floor]} tier this work needs.{" "}
          <Button variant="link-blue" size="inline" className="inline-flex" onClick={onSeeStrengths}>See strengths</Button>
        </p>
      )}
    </div>
  );
}

/** My order: each model dragged by its grip, its effort and menu; then Add model. */
function OrderList({
  name,
  entries,
  route,
  groups,
  onSave,
}: {
  name: string;
  entries: RankedEntry[];
  route: RoutePreview | null;
  groups: readonly ModelGroup[];
  onSave: (entries: RankedEntry[]) => void;
}) {
  const { listRef, shown, dragging, grip } = useDragReorder<RankedEntry, HTMLOListElement>({
    items: entries,
    idOf: placeKey,
    rowSelector: "[data-slot=ranked-place]",
    onMove: (id, to) => {
      const moved = entries.find((entry) => placeKey(entry) === id);
      if (!moved) return;
      const rest = entries.filter((entry) => placeKey(entry) !== id);
      rest.splice(to, 0, moved);
      onSave(rest);
    },
  });
  const taken = entries.map((entry) => entry.target);
  return (
    <div className={cn(HAIRLINE_TOP, "flex flex-col pt-1 pb-2")}>
      <ol ref={listRef} aria-label={`The order ${name} tries`} className="flex flex-col">
        {shown.map((entry, index) => {
          const key = placeKey(entry);
          const replace = (next: RankedEntry | null) =>
            onSave(entries.flatMap((other) => (placeKey(other) === key ? (next ? [next] : []) : [other])));
          return (
            <OrderPlace
              key={key}
              entry={entry}
              position={index + 1}
              count={entries.length}
              taken={taken}
              route={route}
              groups={groups}
              dragging={dragging === key}
              grip={grip(key, index)}
              onEffort={(effort) => replace({ ...entry, effort })}
              onModel={(target) => replace({ target, effort: null })}
              onRemove={() => replace(null)}
            />
          );
        })}
      </ol>
      <DropdownMenu modal={false}>
        <DropdownMenuTrigger asChild>
          <Button variant="link-blue" size="inline" className="text-xs h-9 w-fit gap-2 ps-6.5 font-medium">
            <Plus aria-hidden className="size-3.5" />
            Add model
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="max-h-96 w-xs overflow-y-auto">
          <TargetItems
            onPick={(target) => onSave([...entries, { target, effort: null }])}
            taken={taken}
            candidates={route?.candidates ?? []}
          />
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  );
}

/** One model of an order: grip and place, the model, its effort, and its menu. */
function OrderPlace({
  entry,
  position,
  count,
  taken,
  route,
  groups,
  dragging,
  grip,
  onEffort,
  onModel,
  onRemove,
}: {
  entry: RankedEntry;
  position: number;
  count: number;
  taken: readonly OverrideTarget[];
  route: RoutePreview | null;
  groups: readonly ModelGroup[];
  dragging: boolean;
  grip: ReturnType<ReturnType<typeof useDragReorder>["grip"]>;
  onEffort: (effort: string | null) => void;
  onModel: (target: OverrideTarget) => void;
  onRemove: () => void;
}) {
  const target = entry.target;
  const name = shortPlaceName(target, groups);
  // The effort it runs at: its own, else the one routing would give it now.
  const place = route?.places.find((found) => found.position === position);
  const resolved = place?.model ?? (target.type === "model" ? target.id : null);
  const candidate = route?.candidates.find((found) => found.provider === target.provider && found.model === resolved);
  const effort = entry.effort ?? candidate?.effort ?? null;
  return (
    <li data-slot="ranked-place" className={cn("flex h-11 items-center gap-3 rounded-lg", dragging && "bg-foreground/8")}>
      <button
        type="button"
        aria-label={`Move ${name} (${position} of ${count}); drag, or press up and down`}
        title="Drag to reorder, or focus and press ↑ ↓"
        disabled={count < 2}
        className="flex h-full cursor-grab touch-none items-center gap-3 active:cursor-grabbing disabled:cursor-default"
        {...grip}
      >
        <GripIcon className="text-muted-foreground" />
        <span className="text-xs text-muted-foreground w-2 font-mono">{position}</span>
      </button>
      <span className="flex min-w-0 flex-1 items-center gap-2">
        <ProviderGlyph provider={target.provider} className="shrink-0 size-3.5" />
        <span className="text-label text-foreground truncate">{name}</span>
      </span>
      <DropdownMenu modal={false}>
        <DropdownMenuTrigger asChild>
          <PillTrigger label={`${name}: effort`}>{effort ? `${effortLabel(effort)} effort` : "Auto effort"}</PillTrigger>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-2xs">
          <DropdownMenuRadioGroup value={entry.effort ?? "auto"} onValueChange={(value) => onEffort(value === "auto" ? null : value)}>
            <DropdownMenuRadioItem value="auto" indicator={<Check className="size-icon-md" />}>
              Auto
              <span className="text-muted-foreground ms-auto text-xs">routing picks</span>
            </DropdownMenuRadioItem>
            {effortLevels(target, groups).map((level) => (
              <DropdownMenuRadioItem key={level} value={level} indicator={<Check className="size-icon-md" />}>
                {effortLabel(level)}
              </DropdownMenuRadioItem>
            ))}
          </DropdownMenuRadioGroup>
        </DropdownMenuContent>
      </DropdownMenu>
      <DropdownMenu modal={false}>
        <DropdownMenuTrigger asChild>
          <button
            type="button"
            aria-label={`${name}: actions`}
            className="text-muted-foreground hover:text-foreground flex h-full shrink-0 items-center"
          >
            <DotsHorizontal aria-hidden className="size-3.75" />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-2xs">
          <DropdownMenuLabel>{name}</DropdownMenuLabel>
          <DropdownMenuSub>
            <DropdownMenuSubTrigger>Change model</DropdownMenuSubTrigger>
            <DropdownMenuSubContent className="max-h-96 w-xs overflow-y-auto">
              <TargetItems onPick={onModel} taken={taken} candidates={route?.candidates ?? []} />
            </DropdownMenuSubContent>
          </DropdownMenuSub>
          <DropdownMenuSeparator />
          <DropdownMenuItem onSelect={onRemove}>Remove</DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </li>
  );
}
