import { DotsHorizontal, Plus, Trash, Warning } from "@openai/apps-sdk-ui/components/Icon";

import { useAction } from "@/app/conversation/useAction";
import { useFamilies } from "@/app/routing/RuleForm";
import { type Choice, Segmented, SettingsSelect } from "@/app/settings/parts";
import { HeatBadge } from "@/app/usage/UsagePage";
import { effortLabel, type ModelGroup } from "@/components/assistant-ui/elements/model-selector";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { useDragReorder } from "@/hooks/use-drag-reorder";
import type {
  OverrideTarget,
  RankedEntry,
  RankedPlace,
  Ranking,
  RouteCandidate,
  TaskCategory,
} from "@/ipc/generated";
import {
  CATEGORY_LABELS,
  DEFAULT_FLOORS,
  isFable,
  placeName,
  PROVIDERS,
  sameTarget,
  TIER_LABELS,
  tierBelow,
  VENDOR_LABELS,
} from "@/lib/routing";
import { modelName, useAvailableModelGroups } from "@/lib/setup";
import { cn } from "@/lib/utils";
import { providerOn } from "@/state/providers";
import { changeRanking } from "@/state/routing";
import { useApp } from "@/state/store";

/** The efforts a place may ask for: routing never goes above high. */
const EFFORTS = ["low", "medium", "high"] as const;

/** A place's row key: its position is not stable while it moves, its target is. */
function placeKey(entry: RankedEntry): string {
  const target = entry.target;
  return `${target.type}:${target.provider}:${target.type === "family" ? target.family : target.type === "model" ? target.id : ""}`;
}

/**
 * A Manual ranking: the places routing tries top-down, each with its effort, reordered by the grip
 * (drag, or ↑ and ↓ with the grip focused); then what happens when none of them can run.
 * `places` and `candidates` are the live preview's, when it is about this ranking.
 */
export function ManualList({
  ranking,
  category,
  places,
  candidates,
  groups,
}: {
  ranking: Ranking;
  category: TaskCategory;
  places: readonly RankedPlace[] | null;
  candidates: readonly RouteCandidate[];
  groups: readonly ModelGroup[];
}) {
  const action = useAction();
  const entries = ranking.entries;
  const edit = (change: (ranking: Ranking) => Ranking | null) =>
    action.run(() => changeRanking(ranking.id, change));
  const setEntries = (next: RankedEntry[]) => edit((current) => ({ ...current, entries: next }));
  const { listRef, shown, dragging, grip } = useDragReorder<RankedEntry, HTMLOListElement>({
    items: entries,
    idOf: placeKey,
    rowSelector: "[data-slot=ranked-place]",
    onMove: (id, to) => {
      const moved = entries.find((entry) => placeKey(entry) === id);
      if (!moved) return;
      const rest = entries.filter((entry) => placeKey(entry) !== id);
      rest.splice(to, 0, moved);
      setEntries(rest);
    },
  });

  return (
    <div data-slot="manual-ranking" className="flex flex-col gap-2 px-4 py-3">
      {entries.length === 0 ? (
        ranking.only ? (
          <p className="text-warning text-xs">
            No models, and work waits for these: nothing of this kind can run until you add one.
          </p>
        ) : (
          <p className="text-muted-foreground text-xs">
            No models yet: add the ones routing should try, best first.
          </p>
        )
      ) : (
        <ol ref={listRef} aria-label={`Your ${CATEGORY_LABELS[category]} ranking`} className="flex flex-col">
          {shown.map((entry, index) => {
            const key = placeKey(entry);
            // The live status belongs to the place as saved (not while it is dragged elsewhere).
            const place = dragging ? null : (places?.find((found) => found.position === index + 1) ?? null);
            return (
              <PlaceRow
                key={key}
                entry={entry}
                position={index + 1}
                count={entries.length}
                place={place}
                candidates={candidates}
                category={category}
                groups={groups}
                dragging={dragging === key}
                grip={grip(key, index)}
                onEffort={(effort) =>
                  setEntries(entries.map((other) => (placeKey(other) === key ? { ...other, effort } : other)))
                }
                onRemove={() => setEntries(entries.filter((other) => placeKey(other) !== key))}
              />
            );
          })}
        </ol>
      )}
      <div className="flex flex-wrap items-center justify-between gap-2">
        <AddPlaceMenu
          entries={entries}
          candidates={candidates}
          groups={groups}
          onAdd={(target) => setEntries([...entries, { target, effort: null }])}
        />
        <div className="flex items-center gap-2">
          <span className="text-muted-foreground text-xs">When none can run</span>
          <Segmented
            label="When none of these models can run"
            value={ranking.only ? "wait" : "automatic"}
            options={[
              { value: "automatic", label: "Pick automatically" },
              { value: "wait", label: "Wait for these" },
            ]}
            onChange={(value) => edit((current) => ({ ...current, only: value === "wait" }))}
          />
        </div>
      </div>
      {action.error && (
        <p role="alert" className="text-destructive text-xs">
          {action.error}
        </p>
      )}
    </div>
  );
}

function PlaceRow({
  entry,
  position,
  count,
  place,
  candidates,
  category,
  groups,
  dragging,
  grip,
  onEffort,
  onRemove,
}: {
  entry: RankedEntry;
  position: number;
  count: number;
  place: RankedPlace | null;
  candidates: readonly RouteCandidate[];
  category: TaskCategory;
  groups: readonly ModelGroup[];
  dragging: boolean;
  grip: ReturnType<ReturnType<typeof useDragReorder>["grip"]>;
  onEffort: (effort: string | null) => void;
  onRemove: () => void;
}) {
  const target = entry.target;
  const name = placeName(target, groups);
  // The model the place comes to now, and what routing knows of it.
  const resolved = place?.model ?? (target.type === "model" ? target.id : null);
  const candidate = resolved
    ? candidates.find((found) => found.provider === target.provider && found.model === resolved)
    : undefined;
  const floor = DEFAULT_FLOORS[category];
  const belowFloor = candidate !== undefined && tierBelow(candidate.tier, floor);
  const efforts = effortChoices(target, groups);
  return (
    <li
      data-slot="ranked-place"
      data-dragging={dragging || undefined}
      className={cn(
        "flex items-center gap-2 rounded-control py-1.5",
        dragging && "bg-foreground/5",
        place?.chosen && "text-foreground",
      )}
    >
      <button
        type="button"
        aria-label={`Move #${position}, ${name} (${position} of ${count}); drag, or press up and down`}
        title="Drag to reorder, or focus and press ↑ ↓"
        disabled={count < 2}
        className="text-muted-foreground hover:text-foreground size-icon-button-sm rounded-control grid shrink-0 cursor-grab touch-none place-items-center active:cursor-grabbing disabled:cursor-default disabled:opacity-40"
        {...grip}
      >
        <DotsHorizontal aria-hidden className="size-icon-sm rotate-90" />
      </button>
      <span className="text-muted-foreground w-5 shrink-0 text-xs tabular-nums">#{position}</span>
      <div className="flex min-w-0 flex-1 flex-col">
        <span className="flex min-w-0 items-center gap-1.5">
          <span className="text-label truncate font-medium">{name}</span>
          {place?.chosen && <Badge variant="secondary">Runs it</Badge>}
          {candidate?.new && (
            <Badge variant="warning" title="A new model, not rated here yet">
              New
            </Badge>
          )}
        </span>
        {target.type !== "model" && place?.model && (
          <span className="text-muted-foreground truncate text-xs">
            Now {modelName(groups, { provider: target.provider, model: place.model, effort: null })}
          </span>
        )}
        {place?.why && (
          <span className="text-muted-foreground truncate text-xs" title={place.why}>
            Skipped: {place.why}
          </span>
        )}
        {belowFloor && candidate && (
          <span className="text-warning flex items-center gap-1 text-xs">
            <Warning aria-hidden className="size-icon-xs shrink-0" />
            {candidate.tier === "unrated"
              ? `Not rated yet; routing asks ${TIER_LABELS[floor].toLowerCase()} or better for ${CATEGORY_LABELS[category]}`
              : `${TIER_LABELS[candidate.tier]}: below the ${TIER_LABELS[floor].toLowerCase()} tier routing asks for ${CATEGORY_LABELS[category]}`}
          </span>
        )}
      </div>
      {candidate?.heat && candidate.heat !== "cool" && <HeatBadge heat={candidate.heat} />}
      <SettingsSelect
        label={`Effort for ${name}`}
        value={entry.effort ?? "auto"}
        options={efforts}
        onChange={(value) => onEffort(value === "auto" ? null : value)}
      />
      <Button
        type="button"
        size="icon-xs"
        variant="ghost"
        aria-label={`Remove ${name} from the ranking`}
        className="text-muted-foreground"
        onClick={onRemove}
      >
        <Trash />
      </Button>
    </li>
  );
}

/** Auto, then the levels up to high the model accepts (all three for a family or a vendor). */
function effortChoices(target: OverrideTarget, groups: readonly ModelGroup[]): Choice<string>[] {
  const model =
    target.type === "model"
      ? groups
          .find((group) => group.provider === target.provider)
          ?.models.find((found) => found.id === target.id)
      : undefined;
  // A model without effort levels runs at its own; one the lists don't know takes any.
  const levels = model ? EFFORTS.filter((level) => model.efforts.includes(level)) : EFFORTS;
  return [
    { value: "auto", label: "Auto", hint: "The effort routing picks for this kind of work" },
    ...levels.map((level) => ({ value: level, label: effortLabel(level) })),
  ];
}

/** A model, the newest of a family, or any model of a vendor, from the live lists. */
function AddPlaceMenu({
  entries,
  candidates,
  groups,
  onAdd,
}: {
  entries: readonly RankedEntry[];
  candidates: readonly RouteCandidate[];
  groups: readonly ModelGroup[];
  onAdd: (target: OverrideTarget) => void;
}) {
  const { families } = useFamilies();
  // Only what the user made available is offered; names still come from every list.
  const available = useAvailableModelGroups();
  const availability = useApp((s) => s.settings);
  const listed = (target: OverrideTarget) => entries.some((entry) => sameTarget(entry.target, target));
  return (
    <DropdownMenu modal={false}>
      <DropdownMenuTrigger asChild>
        <Button type="button" size="xs" variant="ghost">
          <Plus />
          Add model
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="max-h-96 w-xs overflow-y-auto">
        {PROVIDERS.filter((provider) => providerOn(availability, provider)).map((provider, index) => {
          const group = available.find((entry) => entry.provider === provider);
          const models = (group?.models ?? []).filter((model) => !model.legacy && !isFable(model));
          const vendor: OverrideTarget = { type: "vendor", provider };
          return (
            <div key={provider}>
              {index > 0 && <DropdownMenuSeparator />}
              <DropdownMenuLabel>{VENDOR_LABELS[provider]}</DropdownMenuLabel>
              {models.map((model) => {
                const target: OverrideTarget = { type: "model", provider, id: model.id };
                const isNew = candidates.some(
                  (candidate) => candidate.provider === provider && candidate.model === model.id && candidate.new,
                );
                return (
                  <DropdownMenuItem key={model.id} disabled={listed(target)} onSelect={() => onAdd(target)}>
                    <span className="truncate">{model.displayName}</span>
                    {isNew && <Badge variant="warning">New</Badge>}
                    <span className="text-muted-foreground ms-auto truncate font-mono text-2xs">
                      {model.id}
                    </span>
                  </DropdownMenuItem>
                );
              })}
              {(families.get(provider) ?? []).map((family) => {
                const target: OverrideTarget = { type: "family", provider, family };
                return (
                  <DropdownMenuItem key={family} disabled={listed(target)} onSelect={() => onAdd(target)}>
                    {placeName(target, groups)}
                    <span className="text-muted-foreground ms-auto text-xs">follows releases</span>
                  </DropdownMenuItem>
                );
              })}
              <DropdownMenuItem disabled={listed(vendor)} onSelect={() => onAdd(vendor)}>
                {placeName(vendor, groups)}
                <span className="text-muted-foreground ms-auto text-xs">best scored</span>
              </DropdownMenuItem>
            </div>
          );
        })}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
