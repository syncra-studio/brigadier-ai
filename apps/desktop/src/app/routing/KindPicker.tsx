import { Check, ChevronDown, Clock } from "@openai/apps-sdk-ui/components/Icon";

import { useAction } from "@/app/conversation/useAction";
import { selectTrigger, SettingsRow } from "@/app/settings/parts";
import { findModel, type ModelGroup } from "@/components/assistant-ui/elements/model-selector";
import { ProviderGlyph } from "@/components/glyphs/provider-glyphs";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { useNow } from "@/hooks/use-now";
import type { ProviderKind, Ranking, RoutePreview, TaskCategory } from "@/ipc/generated";
import { formatCountdown } from "@/lib/format";
import { choiceName, isFable, KIND_HINTS, PLAIN_KIND_NAMES, placeName } from "@/lib/routing";
import { useAvailableModelGroups } from "@/lib/setup";
import { cn } from "@/lib/utils";
import { mayWork } from "@/state/providers";
import { setKindModel } from "@/state/routing";
import { useApp } from "@/state/store";

/** The picker's value for Automatic, and for a list of several models (set under Advanced). */
const AUTOMATIC = "automatic";
const LIST = "list";

const modelKey = (provider: ProviderKind, id: string) => `${provider}/${id}`;

/** The kind of work's own ranking everywhere, if the user has one. */
function everywhereRanking(rankings: readonly Ranking[], category: TaskCategory): Ranking | null {
  return (
    rankings.find(
      (ranking) =>
        ranking.category === category && ranking.projectId === null && ranking.areas.length === 0,
    ) ?? null
  );
}

/**
 * One kind of work in Providers' "Who does what": its name in plain words, what it does, and
 * a picker: Automatic (Brigadier picks; what it would pick now shows) or one model the user
 * chose. A chosen model that can't take a task (out of quota) hands it to Automatic, and the row
 * says so.
 */
export function KindRow({
  category,
  route,
  groups,
}: {
  category: TaskCategory;
  route: RoutePreview | null;
  groups: readonly ModelGroup[];
}) {
  const rankings = useApp((s) => s.settings.routingRankings);
  const rules = useApp((s) => s.settings.routingOverrides);
  const available = useAvailableModelGroups();
  const action = useAction();
  const now = useNow(30_000);
  const ranking = everywhereRanking(rankings, category);
  const manual = ranking?.manual === true && ranking.entries.length > 0;
  const single = manual && ranking.entries.length === 1 ? ranking.entries[0] : undefined;
  const value = !manual
    ? AUTOMATIC
    : single?.target.type === "model"
      ? modelKey(single.target.provider, single.target.id)
      : LIST;

  const outcome = route?.outcome ?? null;
  const next = outcome?.type === "chosen" ? outcome.choice : null;
  // The logo names the agent, so the picker shows the model's own name ("GPT-6-Luna").
  const nextName = next
    ? (findModel(groups, next)?.displayName ?? choiceName(groups, { ...next, effort: null }))
    : null;
  const pickedName =
    single?.target.type === "model"
      ? (findModel(groups, { provider: single.target.provider, model: single.target.id, effort: null })
          ?.displayName ?? placeName(single.target, groups))
      : single
        ? placeName(single.target, groups)
        : null;
  // The chosen model is out of the running for now: say what takes its tasks meanwhile.
  const standIn =
    single?.target.type === "model" &&
    next !== null &&
    (next.provider !== single.target.provider || next.model !== single.target.id)
      ? nextName
      : null;

  let note: string | null = null;
  if (outcome?.type === "wait") {
    note = `Tasks wait${outcome.resetsAtMs !== null ? ` ${formatCountdown(outcome.resetsAtMs, now)}` : ""}: ${outcome.reason}`;
  } else if (standIn && pickedName) {
    note = `${pickedName} can't take tasks right now, so ${standIn} does until it can`;
  }

  const trigger = (() => {
    if (value === AUTOMATIC) {
      return (
        <>
          <span className="text-muted-foreground shrink-0">Automatic</span>
          {next && nextName && (
            <>
              <span aria-hidden className="text-muted-foreground">
                ·
              </span>
              <ProviderGlyph provider={next.provider} className="size-icon-sm shrink-0" />
              <span className="truncate">{nextName}</span>
            </>
          )}
        </>
      );
    }
    if (value === LIST) {
      return <span className="truncate">Your list · {ranking?.entries.length} models</span>;
    }
    return (
      <>
        {single && <ProviderGlyph provider={single.target.provider} className="size-icon-sm shrink-0" />}
        <span className="truncate">{pickedName}</span>
      </>
    );
  })();

  return (
    <SettingsRow
      label={PLAIN_KIND_NAMES[category]}
      description={
        <>
          {KIND_HINTS[category]}
          {note && (
            <span className="text-warning mt-0.5 flex items-start gap-1">
              <Clock aria-hidden className="size-icon-xs mt-0.5 shrink-0" />
              {note}
            </span>
          )}
        </>
      }
      error={action.error}
    >
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <button
            type="button"
            aria-label={`${PLAIN_KIND_NAMES[category]}: which model`}
            disabled={action.busy}
            className={cn(selectTrigger, "flex w-56 gap-1.5 text-start")}
          >
            <span className="flex min-w-0 flex-1 items-center gap-1.5">{trigger}</span>
            <ChevronDown aria-hidden className="text-muted-foreground size-icon-xs shrink-0" />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-xs max-h-(--radix-dropdown-menu-content-available-height) overflow-y-auto">
          <DropdownMenuRadioGroup
            value={value}
            onValueChange={(picked) => {
              if (picked === LIST) return;
              if (picked === AUTOMATIC) {
                action.run(() => setKindModel(category, null));
                return;
              }
              const [provider, ...rest] = picked.split("/");
              action.run(() =>
                setKindModel(category, { provider: provider as ProviderKind, id: rest.join("/") }),
              );
            }}
          >
            <DropdownMenuRadioItem
              value={AUTOMATIC}
              indicator={<Check className="size-icon-md" />}
              className="h-auto items-start py-1.5"
            >
              <span className="flex min-w-0 flex-col">
                <span>Automatic</span>
                <span className="text-muted-foreground text-xs whitespace-normal">
                  Brigadier picks the best model with quota to spare
                  {value === AUTOMATIC && nextName ? ` (now ${nextName})` : ""}.
                </span>
              </span>
            </DropdownMenuRadioItem>
            {value === LIST && (
              <DropdownMenuRadioItem
                value={LIST}
                indicator={<Check className="size-icon-md" />}
                className="h-auto items-start py-1.5"
              >
                <span className="flex min-w-0 flex-col">
                  <span>Your list · {ranking?.entries.length} models</span>
                  <span className="text-muted-foreground text-xs whitespace-normal">
                    Tried in order; edit it under Advanced. Picking one model replaces it.
                  </span>
                </span>
              </DropdownMenuRadioItem>
            )}
            {available.map((group) => {
              // Only models Brigadier may give work to (the switches above) can be picked.
              const models = group.models.filter(
                (model) =>
                  !model.legacy &&
                  !isFable(model) &&
                  mayWork(rules, { provider: group.provider, id: model.id }),
              );
              if (models.length === 0) return null;
              return (
                <div key={group.provider}>
                  <DropdownMenuSeparator />
                  <DropdownMenuLabel className="flex items-center gap-1.5">
                    <ProviderGlyph provider={group.provider} className="size-icon-sm" />
                    {group.label}
                    {group.unavailable && (
                      <span className="font-normal normal-case">· {group.unavailable}</span>
                    )}
                  </DropdownMenuLabel>
                  {models.map((model) => (
                    <DropdownMenuRadioItem
                      key={model.id}
                      value={modelKey(group.provider, model.id)}
                      disabled={group.unavailable !== null}
                      indicator={<Check className="size-icon-md" />}
                    >
                      {model.displayName}
                    </DropdownMenuRadioItem>
                  ))}
                </div>
              );
            })}
          </DropdownMenuRadioGroup>
          <p className="text-muted-foreground px-2 py-1.5 text-xs whitespace-normal">
            A model you pick that's out of quota hands its tasks to Automatic until it's back.
          </p>
        </DropdownMenuContent>
      </DropdownMenu>
    </SettingsRow>
  );
}
