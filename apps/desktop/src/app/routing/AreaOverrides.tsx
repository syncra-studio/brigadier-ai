import { DotsHorizontal, Plus } from "@openai/apps-sdk-ui/components/Icon";

import { useAction } from "@/app/conversation/useAction";
import { ChipSelect, ChipTrigger, TargetItems } from "@/app/routing/pickers";
import { noneCan, placeKey } from "@/app/routing/TryOrder";
import { areaOverrides, rowRanking, shortPlaceName } from "@/app/routing/words";
import type { ModelGroup } from "@/components/assistant-ui/elements/model-selector";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import type { Area, OverrideTarget, Ranking, RoutePreview, TaskCategory } from "@/ipc/generated";
import { AREA_LABELS, AREAS, joinWords, PLAIN_KIND_NAMES } from "@/lib/routing";
import { cn } from "@/lib/utils";
import { changeRanking, newRankingId } from "@/state/routing";
import { useApp } from "@/state/store";

/** The places a new override starts with when the kind has no order: what routing picks first. */
const SEEDED_PLACES = 2;

/**
 * Area overrides: for tasks of a kind touching some areas, an order of their own, each as a
 * sentence ("In Brigadier, Use apps touching [frontend] tries [GPT-6], then [Opus 5.5], and
 * [waits] if neither can run."). Each can be turned off (its order is kept) or removed.
 */
export function AreaOverrides({
  category,
  scope,
  route,
  groups,
}: {
  category: TaskCategory;
  scope: string | null;
  route: RoutePreview | null;
  groups: readonly ModelGroup[];
}) {
  const rankings = useApp((s) => s.settings.routingRankings);
  const overrides = areaOverrides(rankings, category, scope);
  const row = rowRanking(rankings, category, scope);
  const action = useAction();
  const add = (area: Area) =>
    action.run(() =>
      changeRanking("", (created) => created, () => ({
        id: newRankingId(),
        category,
        areas: [area],
        projectId: scope,
        manual: true,
        // The kind's own order, else the models routing picks first now.
        entries: row?.entries.length
          ? row.entries
          : (route?.candidates ?? [])
              .filter((candidate) => candidate.blocked === null)
              .slice(0, SEEDED_PLACES)
              .map((candidate) => ({
                target: { type: "model", provider: candidate.provider, id: candidate.model },
                effort: null,
              })),
        only: false,
        updatedAtMs: Date.now(),
      })),
    );
  return (
    <section data-setting="Area overrides" data-slot="area-overrides" className="flex flex-col gap-2">
      <div className="flex items-center justify-between gap-2">
        <h3 className="text-sm font-medium">Area overrides</h3>
        <DropdownMenu modal={false}>
          <DropdownMenuTrigger asChild>
            <button type="button" className="text-link flex items-center gap-1 text-xs hover:underline">
              <Plus aria-hidden className="size-icon-xs" />
              Add override
            </button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            <DropdownMenuLabel>An order for tasks touching</DropdownMenuLabel>
            {AREAS.map((area) => (
              <DropdownMenuItem key={area} onSelect={() => add(area)}>
                {AREA_LABELS[area]}
              </DropdownMenuItem>
            ))}
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
      {overrides.length === 0 && (
        <p className="text-muted-foreground text-xs">
          None: tasks touching any area follow the order above.
        </p>
      )}
      {overrides.map((override) => (
        <AreaOverride key={override.id} ranking={override} category={category} groups={groups} />
      ))}
      {action.error && (
        <p role="alert" className="text-destructive text-xs">
          {action.error}
        </p>
      )}
    </section>
  );
}

function AreaOverride({
  ranking,
  category,
  groups,
}: {
  ranking: Ranking;
  category: TaskCategory;
  groups: readonly ModelGroup[];
}) {
  const projects = useApp((s) => s.projects);
  const action = useAction();
  const edit = (change: (ranking: Ranking) => Ranking | null) => action.run(() => changeRanking(ranking.id, change));
  const entries = ranking.entries;
  const setEntries = (next: Ranking["entries"]) => edit((current) => ({ ...current, entries: next }));
  const where = ranking.projectId ? (projects[ranking.projectId]?.name ?? "a removed project") : "All projects";
  const areas = joinWords(ranking.areas.map((area) => AREA_LABELS[area]), "or");
  return (
    <div
      data-slot="area-override"
      data-off={!ranking.manual || undefined}
      className="bg-card border-divider rounded-settings flex items-start gap-2 border px-4 py-3 text-sm"
    >
      <p className={cn("flex min-w-0 flex-1 flex-wrap items-center gap-x-1.5 gap-y-2", !ranking.manual && "opacity-60")}>
        In {where}, {PLAIN_KIND_NAMES[category]} touching
        <DropdownMenu modal={false}>
          <DropdownMenuTrigger asChild>
            <ChipTrigger label="The areas this order is for">{areas}</ChipTrigger>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="start">
            {AREAS.map((area) => (
              <DropdownMenuCheckboxItem
                key={area}
                checked={ranking.areas.includes(area)}
                onSelect={(event) => event.preventDefault()}
                onCheckedChange={(checked) => {
                  const next = checked ? [...ranking.areas, area] : ranking.areas.filter((other) => other !== area);
                  if (next.length > 0) edit((current) => ({ ...current, areas: next }));
                }}
              >
                {AREA_LABELS[area]}
              </DropdownMenuCheckboxItem>
            ))}
          </DropdownMenuContent>
        </DropdownMenu>
        tries
        {entries.map((entry, index) => {
          const key = placeKey(entry);
          const replace = (target: OverrideTarget) =>
            setEntries(entries.map((other) => (placeKey(other) === key ? { target, effort: null } : other)));
          return (
            <span key={key} className="contents">
              {index > 0 && <>{index === entries.length - 1 ? ", then" : ","}</>}
              <DropdownMenu modal={false}>
                <DropdownMenuTrigger asChild>
                  <ChipTrigger label={`${shortPlaceName(entry.target, groups)}: change or remove`}>
                    {shortPlaceName(entry.target, groups)}
                  </ChipTrigger>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="start" className="max-h-96 w-xs overflow-y-auto">
                  <TargetItems onPick={replace} taken={entries.map((other) => other.target)} />
                  <DropdownMenuSeparator />
                  <DropdownMenuItem onSelect={() => setEntries(entries.filter((other) => placeKey(other) !== key))}>
                    Remove
                  </DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
            </span>
          );
        })}
        <DropdownMenu modal={false}>
          <DropdownMenuTrigger asChild>
            <Button type="button" size="icon-xs" variant="ghost" aria-label="Add a model" className="text-link">
              <Plus />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="start" className="max-h-96 w-xs overflow-y-auto">
            <TargetItems
              onPick={(target) => setEntries([...entries, { target, effort: null }])}
              taken={entries.map((other) => other.target)}
            />
          </DropdownMenuContent>
        </DropdownMenu>
        , and
        <ChipSelect
          label="When none of these models can run"
          value={ranking.only ? "wait" : "automatic"}
          options={[
            { value: "wait", label: "waits" },
            { value: "automatic", label: "picks automatically" },
          ]}
          onChange={(value) => edit((current) => ({ ...current, only: value === "wait" }))}
        />
        if {noneCan(entries.length)} run.
        {!ranking.manual && <span className="text-muted-foreground basis-full text-xs">Off: these tasks follow the order above.</span>}
      </p>
      <DropdownMenu modal={false}>
        <DropdownMenuTrigger asChild>
          <Button type="button" size="icon-xs" variant="ghost" aria-label="This override: actions" className="text-muted-foreground shrink-0">
            <DotsHorizontal />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          <DropdownMenuItem onSelect={() => edit((current) => ({ ...current, manual: !current.manual }))}>
            {ranking.manual ? "Turn off" : "Turn on"}
          </DropdownMenuItem>
          <DropdownMenuItem onSelect={() => edit(() => null)}>Remove</DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      {action.error && (
        <p role="alert" className="text-destructive text-xs">
          {action.error}
        </p>
      )}
    </div>
  );
}
