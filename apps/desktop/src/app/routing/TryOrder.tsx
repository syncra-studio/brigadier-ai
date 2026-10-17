import { Check, ChevronDown, Plus, X } from "@openai/apps-sdk-ui/components/Icon";

import { useAction } from "@/app/conversation/useAction";
import { TargetItems } from "@/app/routing/pickers";
import { authorPicks, decides, orderSummary, shortModelName, shortPlaceName } from "@/app/routing/words";
import { Segmented, SettingsSelect } from "@/app/settings/parts";
import { effortLabel, type ModelGroup } from "@/components/assistant-ui/elements/model-selector";
import { ProviderGlyph } from "@/components/glyphs/provider-glyphs";
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
import { useDragReorder } from "@/hooks/use-drag-reorder";
import type { OverrideTarget, RankedEntry, Ranking, RoutePreview, TaskCategory } from "@/ipc/generated";
import { PLAIN_KIND_NAMES } from "@/lib/routing";
import { cn } from "@/lib/utils";
import { changeRanking, newRankingId } from "@/state/routing";

/** The efforts a place may ask for: routing never goes above high. */
const EFFORTS = ["low", "medium", "high"] as const;

/** What happens when none of a list's models can run. */
export const WHEN_NONE: { value: "automatic" | "wait"; label: string }[] = [
  { value: "automatic", label: "pick automatically" },
  { value: "wait", label: "wait" },
];

/** "it can't", "neither can", "none can": a list's models in "If … run". */
export function noneCan(count: number): string {
  if (count === 1) return "it can't";
  return count === 2 ? "neither can" : "none can";
}

/** A place's key: its position moves while it is dragged, its target doesn't. */
export function placeKey(entry: RankedEntry): string {
  const target = entry.target;
  return `${target.type}:${target.provider}:${target.type === "family" ? target.family : target.type === "model" ? target.id : ""}`;
}

/** Auto, then the levels up to high the model accepts (all three for a family or a vendor). */
function effortLevels(target: OverrideTarget, groups: readonly ModelGroup[]): readonly string[] {
  const model =
    target.type === "model"
      ? groups.find((group) => group.provider === target.provider)?.models.find((found) => found.id === target.id)
      : undefined;
  return model ? EFFORTS.filter((level) => model.efforts.includes(level)) : EFFORTS;
}

/**
 * A kind of work opened in place: Automatic or My order. Its order is a list of models: each
 * dragged by its grip to reorder, × removes it, and its menu changes its effort or model; then
 * Add model and what happens when none can run. A project without an order of its own follows
 * All projects'. Under it, what it runs on right now, with Why for the whole ranking.
 */
export function TryOrder({
  category,
  scope,
  own,
  everywhere,
  route,
  groups,
  onWhy,
}: {
  category: TaskCategory;
  scope: string | null;
  /** The kind's own row in this scope. */
  own: Ranking | null;
  /** In a project: the kind's row in All projects. */
  everywhere: Ranking | null;
  route: RoutePreview | null;
  groups: readonly ModelGroup[];
  /** Shows the ranking right now behind the pick. */
  onWhy: () => void;
}) {
  const action = useAction();
  const name = PLAIN_KIND_NAMES[category];
  // A project without a row of its own follows All projects' list; one set to Automatic doesn't.
  const follows = own === null && scope !== null && decides(everywhere) ? everywhere : null;
  const deciding = decides(own) ? own : null;
  const entries = deciding?.entries ?? [];
  const created = (manual: boolean, start: RankedEntry[]): Ranking => ({
    id: newRankingId(),
    category,
    areas: [],
    projectId: scope,
    manual,
    entries: start,
    only: follows?.only ?? false,
    updatedAtMs: Date.now(),
  });

  const save = (next: RankedEntry[], only?: boolean) =>
    action.run(() =>
      changeRanking(
        own?.id ?? "",
        // Removing the last model leaves the kind on Automatic; the empty list is kept.
        (current) => ({ ...current, entries: next, manual: next.length > 0, only: only ?? current.only }),
        () => created(true, []),
      ),
    );
  const add = (target: OverrideTarget) =>
    save([...(deciding ? entries : (follows?.entries ?? [])), { target, effort: null }]);
  // Automatic keeps the order to come back to; My order brings it back, else starts one with
  // All projects' order or the model picked now.
  const choose = (mode: "automatic" | "order") => {
    if (mode === "automatic") {
      return action.run(() =>
        changeRanking(own?.id ?? "", (current) => ({ ...current, manual: false }), () => created(false, [])),
      );
    }
    if (own && own.entries.length > 0) {
      return action.run(() => changeRanking(own.id, (current) => ({ ...current, manual: true })));
    }
    const picked = route?.outcome.type === "chosen" ? route.outcome.choice : null;
    const start: RankedEntry[] =
      follows?.entries ??
      (picked ? [{ target: { type: "model", provider: picked.provider, id: picked.model ?? "" }, effort: null }] : []);
    return save(start);
  };

  const { listRef, shown, dragging, grip } = useDragReorder<RankedEntry, HTMLOListElement>({
    items: entries,
    idOf: placeKey,
    rowSelector: "[data-slot=ranked-place]",
    onMove: (id, to) => {
      const moved = entries.find((entry) => placeKey(entry) === id);
      if (!moved) return;
      const rest = entries.filter((entry) => placeKey(entry) !== id);
      rest.splice(to, 0, moved);
      save(rest);
    },
  });

  const addButton = (
    <DropdownMenu modal={false}>
      <DropdownMenuTrigger asChild>
        <button type="button" className="text-link flex items-center gap-1.5 text-label hover:underline">
          <Plus aria-hidden className="size-icon-sm" />
          Add model
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="max-h-96 w-xs overflow-y-auto">
        <TargetItems
          onPick={add}
          taken={(deciding ? entries : (follows?.entries ?? [])).map((entry) => entry.target)}
          candidates={route?.candidates ?? []}
        />
      </DropdownMenuContent>
    </DropdownMenu>
  );

  return (
    <div data-slot="try-order" className="flex flex-col gap-3">
      <Segmented
        label={`How ${name} picks a model`}
        // A project following All projects' order is on an order; Automatic gives it its own.
        value={deciding || follows ? "order" : "automatic"}
        options={[
          { value: "automatic", label: "Automatic" },
          { value: "order", label: "My order" },
        ]}
        disabled={action.busy}
        onChange={choose}
      />
      {deciding ? (
        <>
          <ol ref={listRef} aria-label={`The order ${name} tries`} className="flex flex-col gap-1.5">
            {shown.map((entry, index) => {
              const key = placeKey(entry);
              return (
                <OrderPlace
                  key={key}
                  entry={entry}
                  position={index + 1}
                  count={entries.length}
                  taken={entries.map((other) => other.target)}
                  route={route}
                  groups={groups}
                  dragging={dragging === key}
                  grip={grip(key, index)}
                  onEffort={(effort) =>
                    save(entries.map((other) => (placeKey(other) === key ? { ...other, effort } : other)))
                  }
                  onModel={(target) =>
                    save(entries.map((other) => (placeKey(other) === key ? { target, effort: null } : other)))
                  }
                  onRemove={() => save(entries.filter((other) => placeKey(other) !== key))}
                />
              );
            })}
          </ol>
          <div className="flex flex-wrap items-center justify-between gap-3">
            {addButton}
            <span className="text-foreground/65 flex items-center gap-2 text-xs">
              If {noneCan(entries.length)} run
              <SettingsSelect
                label={`When none of ${name}'s models can run`}
                value={deciding.only ? "wait" : "automatic"}
                options={WHEN_NONE}
                onChange={(value) => save(entries, value === "wait")}
              />
            </span>
          </div>
        </>
      ) : (
        <div className="text-foreground/65 flex flex-wrap items-center justify-between gap-3 text-xs">
          {follows ? (
            <span>
              Follows All projects: {orderSummary(follows, groups)}.
            </span>
          ) : (
            <span>
              Picks the strongest model that can run now
              {/* A stand-in takes over when the chat's model hits its limit. */}
              {category === "chat" && ", never the chat's own model"}.
            </span>
          )}
          {addButton}
        </div>
      )}
      <NowPicked route={route} groups={groups} deciding={deciding !== null} onWhy={onWhy} />
      {action.error && (
        <p role="alert" className="text-destructive text-xs">
          {action.error}
        </p>
      )}
    </div>
  );
}

/**
 * "Right now: Opus 5.5 · high, the first in your order that can run. Why", or that the next task
 * waits; for a review, each vendor's change with the other vendor's pick.
 */
function NowPicked({
  route,
  groups,
  deciding,
  onWhy,
}: {
  route: RoutePreview | null;
  groups: readonly ModelGroup[];
  deciding: boolean;
  onWhy: () => void;
}) {
  if (!route) return null;
  const picks = authorPicks(route, groups);
  const outcome = route.outcome;
  let now: React.ReactNode;
  if (picks) {
    // Automatic reviews go to the other vendor from the author; an order may not.
    now = (
      <>
        <span className="text-foreground">Right now:</span>{" "}
        {deciding ? picks : `the other vendor from the author reviews — ${picks}`}.
      </>
    );
  } else if (outcome.type === "wait") {
    now = <span className="text-warning">Right now: the next task waits.</span>;
  } else {
    now = (
      <>
        <span className="text-foreground">
          Right now: {shortModelName(groups, outcome.choice.provider, outcome.choice.model ?? "")}
          {outcome.choice.effort && ` · ${outcome.choice.effort}`}
        </span>
        {!deciding
          ? "."
          : route.places.some((place) => place.chosen)
            ? ", the first in your order that can run."
            : ", picked automatically: none in your order can run."}
      </>
    );
  }
  return (
    <p data-slot="now-picked" className="text-foreground/65 border-divider border-t pt-3 text-xs">
      {now}{" "}
      <button type="button" className="text-link hover:underline" onClick={onWhy}>
        Why
      </button>
    </p>
  );
}

/** One model of the order as a row: grip and place, the model, its effort and menu, and ×. */
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
  /** Every place's target: none can be picked again. */
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
  const candidate = route?.candidates.find(
    (found) => found.provider === target.provider && found.model === resolved,
  );
  const effort = entry.effort ?? candidate?.effort ?? null;
  return (
    <li
      data-slot="ranked-place"
      className={cn(
        "border-divider bg-card rounded-nav flex h-9 items-center gap-2 border text-sm",
        dragging && "bg-foreground/10",
      )}
    >
      <button
        type="button"
        aria-label={`Move ${name} (${position} of ${count}); drag, or press up and down`}
        title="Drag to reorder, or focus and press ↑ ↓"
        disabled={count < 2}
        className="text-muted-foreground hover:text-foreground flex h-full cursor-grab touch-none items-center gap-2 ps-3 pe-1 font-mono text-xs active:cursor-grabbing disabled:cursor-default"
        {...grip}
      >
        <span aria-hidden>⠿</span>
        {position}
      </button>
      <ProviderGlyph provider={target.provider} className="size-icon-sm shrink-0" />
      <span className="min-w-0 flex-1 truncate">{name}</span>
      <DropdownMenu modal={false}>
        <DropdownMenuTrigger asChild>
          <button
            type="button"
            aria-label={`${name}: change its model or effort`}
            className="text-muted-foreground hover:text-foreground flex h-full items-center gap-1 px-1.5 text-xs"
          >
            {effort ? `${effort} effort` : "auto effort"}
            <ChevronDown aria-hidden className="size-icon-xs" />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-2xs">
          <DropdownMenuLabel>Effort</DropdownMenuLabel>
          <DropdownMenuRadioGroup
            value={entry.effort ?? "auto"}
            onValueChange={(value) => onEffort(value === "auto" ? null : value)}
          >
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
          <DropdownMenuSeparator />
          <DropdownMenuSub>
            <DropdownMenuSubTrigger>Change model</DropdownMenuSubTrigger>
            <DropdownMenuSubContent className="max-h-96 w-xs overflow-y-auto">
              <TargetItems onPick={onModel} taken={taken} candidates={route?.candidates ?? []} />
            </DropdownMenuSubContent>
          </DropdownMenuSub>
          <DropdownMenuItem onSelect={onRemove}>Remove</DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      <button
        type="button"
        aria-label={`Remove ${name}`}
        className="text-muted-foreground hover:text-foreground flex h-full items-center pe-3 ps-1"
        onClick={onRemove}
      >
        <X aria-hidden className="size-icon-xs" />
      </button>
    </li>
  );
}
