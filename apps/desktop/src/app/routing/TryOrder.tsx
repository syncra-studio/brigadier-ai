import { Check, ChevronDown, Plus, X } from "@openai/apps-sdk-ui/components/Icon";

import { useAction } from "@/app/conversation/useAction";
import { ChipSelect, TargetItems } from "@/app/routing/pickers";
import { authorPicks, decides, orderSummary, shortModelName, shortPlaceName } from "@/app/routing/words";
import { effortLabel, type ModelGroup } from "@/components/assistant-ui/elements/model-selector";
import { ProviderGlyph } from "@/components/glyphs/provider-glyphs";
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
 * A kind of work's order in a scope, as a sentence: "Review changes tries [1 Opus 5.5 · high],
 * then [2 GPT-6 · high] [+ Add model]. If neither can run, [pick automatically]." Each model is a
 * chip: dragged by its grip to reorder, × removes it, and its menu changes its effort or model.
 * Without an order of its own, the kind picks automatically (or follows All projects), and
 * adding a model starts one.
 */
export function TryOrder({
  category,
  scope,
  own,
  everywhere,
  route,
  groups,
}: {
  category: TaskCategory;
  scope: string | null;
  /** The kind's own row in this scope. */
  own: Ranking | null;
  /** In a project: the kind's row in All projects. */
  everywhere: Ranking | null;
  route: RoutePreview | null;
  groups: readonly ModelGroup[];
}) {
  const action = useAction();
  const name = PLAIN_KIND_NAMES[category];
  // A project without a row of its own follows All projects' list; one set to Automatic doesn't.
  const follows = own === null && scope !== null && decides(everywhere) ? everywhere : null;
  const deciding = decides(own) ? own : null;
  const entries = deciding?.entries ?? [];

  const save = (next: RankedEntry[], only?: boolean) =>
    action.run(() =>
      changeRanking(
        own?.id ?? "",
        // Removing the last model leaves the kind on Automatic; the empty list is kept.
        (current) => ({ ...current, entries: next, manual: next.length > 0, only: only ?? current.only }),
        () => ({
          id: newRankingId(),
          category,
          areas: [],
          projectId: scope,
          manual: true,
          entries: [],
          only: follows?.only ?? false,
          updatedAtMs: Date.now(),
        }),
      ),
    );
  const add = (target: OverrideTarget) =>
    save([...(deciding ? entries : (follows?.entries ?? [])), { target, effort: null }]);
  const bringBack = () =>
    own && action.run(() => changeRanking(own.id, (current) => ({ ...current, manual: true })));

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
        <Button type="button" size="sm" variant="outline" className="text-link rounded-nav h-8 font-normal">
          <Plus />
          Add model
        </Button>
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
    <div
      data-slot="try-order"
      className="bg-card border-divider rounded-settings flex flex-col gap-3 border px-5 py-4 text-base"
    >
      {deciding ? (
        <>
          <ol ref={listRef} aria-label={`The order ${name} tries`} className="flex flex-col gap-2.5">
            {shown.map((entry, index) => {
              const key = placeKey(entry);
              return (
                <li key={key} data-slot="ranked-place" className="flex flex-wrap items-center gap-2">
                  <span>{index === 0 ? `${name} tries` : "then"}</span>
                  <OrderChip
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
                  {index === shown.length - 1 && addButton}
                </li>
              );
            })}
          </ol>
          <p className="flex flex-wrap items-center gap-2">
            If {noneCan(entries.length)} run,
            <span className="inline-flex items-center gap-0.5">
              <ChipSelect
                large
                label={`When none of ${name}'s models can run`}
                value={deciding.only ? "wait" : "automatic"}
                options={WHEN_NONE}
                onChange={(value) => save(entries, value === "wait")}
              />
              .
            </span>
          </p>
          <p className="text-muted-foreground flex flex-wrap gap-x-5 gap-y-1 text-2xs">
            <span>⠿ Drag to reorder</span>
            <span>× Remove a model</span>
            <span>⌄ Change model or effort</span>
          </p>
        </>
      ) : (
        <>
          <p className="flex flex-wrap items-center gap-2">
            {follows ? (
              <>
                {name} follows All projects: {orderSummary(follows, groups)}.
              </>
            ) : (
              <>
                {name} picks automatically
                {/* A stand-in takes over when the chat's model hits its limit. */}
                {category === "chat" && ", never the chat's own model"}
                <NowPicked route={route} groups={groups} />
              </>
            )}
            {addButton}
          </p>
          {own && own.entries.length > 0 && !own.manual && (
            <p className="text-muted-foreground text-xs">
              <button type="button" className="text-link hover:underline" onClick={bringBack}>
                Bring back your order
              </button>
              : {orderSummary(own, groups)}
            </p>
          )}
        </>
      )}
      {action.error && (
        <p role="alert" className="text-destructive text-xs">
          {action.error}
        </p>
      )}
    </div>
  );
}

/**
 * " — now Opus 5.5 · high", or that the next task waits; for a review, each vendor's change
 * with the other vendor's pick.
 */
function NowPicked({ route, groups }: { route: RoutePreview | null; groups: readonly ModelGroup[] }) {
  if (!route) return null;
  const picks = authorPicks(route, groups);
  if (picks) {
    return (
      <span className="text-muted-foreground">
        , adversarial: the other vendor from the author reviews — {picks}
      </span>
    );
  }
  const outcome = route.outcome;
  if (outcome.type === "wait") return <span className="text-warning"> — the next task waits</span>;
  return (
    <span className="text-muted-foreground">
      {" "}
      — now {shortModelName(groups, outcome.choice.provider, outcome.choice.model ?? "")}
      {outcome.choice.effort && ` · ${outcome.choice.effort}`}
    </span>
  );
}

/** One model of the order: grip and place, the model and its effort, its menu and ×. */
function OrderChip({
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
    <span
      className={cn(
        "border-divider bg-foreground/5 rounded-nav inline-flex h-8 items-center border text-sm",
        dragging && "bg-foreground/10",
      )}
    >
      <button
        type="button"
        aria-label={`Move ${name} (${position} of ${count}); drag, or press up and down`}
        title="Drag to reorder, or focus and press ↑ ↓"
        disabled={count < 2}
        className="text-muted-foreground hover:text-foreground flex h-full cursor-grab touch-none items-center gap-1.5 ps-2 pe-1 font-mono text-2xs active:cursor-grabbing disabled:cursor-default"
        {...grip}
      >
        <span aria-hidden>⠿</span>
        {position}
      </button>
      <DropdownMenu modal={false}>
        <DropdownMenuTrigger asChild>
          <button
            type="button"
            aria-label={`${name}: change its model or effort`}
            className="hover:bg-foreground/5 flex h-full items-center gap-1.5 px-1.5"
          >
            <ProviderGlyph provider={target.provider} className="size-icon-sm" />
            {name}
            <span className="text-muted-foreground text-xs">· {effort ?? "auto"}</span>
            <ChevronDown aria-hidden className="text-muted-foreground size-icon-xs" />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="w-2xs">
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
        className="text-muted-foreground hover:text-foreground flex h-full items-center pe-2 ps-0.5"
        onClick={onRemove}
      >
        <X aria-hidden className="size-icon-xs" />
      </button>
    </span>
  );
}
