import { type KeyboardEvent, type ReactNode, useEffect, useId, useRef, useState } from "react";

import { Button } from "@/components/controls/button";
import { Switch } from "@/components/controls/switch";
import { Bolt, Check, ChevronRight, MagnifyingGlassSmSearch, ProviderGlyph, Sparkles, Star, StarFilled } from "@/components/icons";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/overlays/popover";
import type { ModelGroup } from "@/components/shared/model-selector";
import type { ModelInfo, ProviderKind } from "@/ipc/generated";
import { PLAIN_FIELD } from "@/lib/plainTyping";
import { cliFacts } from "@/lib/providerKinds";
import { shippedModelNames } from "@/lib/shippedModels";
import { cn } from "@/lib/utils";
import { openSettings } from "@/state/actions";
import { setModelFavorite } from "@/state/providers";
import { useApp } from "@/state/store";

/*
 * The model picker: a search across every agent on top, a sidebar of Favorites and the agents
 * beside the browsed one's models, and a footer with the effort of the model in hand, all on one
 * surface in one popup of a fixed height. Every place that picks a model opens this one panel;
 * each use switches on only the parts it needs. A pick keeps it open and moves the keyboard to
 * the efforts.
 */

/** A row the picker checks: an agent's model by id, or (`model` null) the agent's own default. */
export type PickerChoice = { provider: ProviderKind; model: string | null };

/** What a press in the footer picks along with a model not yet picked. */
export type PickedWith = { effort?: string; fast?: boolean };

export type ModelPickerProps = {
  groups: readonly ModelGroup[];
  /** The row that is checked, if any. */
  checked: PickerChoice | null;
  /** The agent it opens on when no row is checked (the first otherwise). */
  opensOn?: ProviderKind | undefined;
  /** Only ever one agent's models: it opens there, not on Favorites. */
  locked?: boolean | undefined;
  /** A model picked (`model` null: the agent's default row), with what was pressed to pick it. */
  onPick: (provider: ProviderKind, model: ModelInfo | null, also?: PickedWith) => void;
  /** The checked model's effort. */
  effort?: { value: string | null; onChange: (level: string) => void } | undefined;
  /** Why the rules keep one of a model's efforts out, in their words; it shows greyed with them. */
  refused?: ((provider: ProviderKind, model: ModelInfo, level: string) => string | null) | undefined;
  /** The checked model's fast tier; no footer Speed without it. */
  fast?: { on: boolean; onChange: (on: boolean) => void } | undefined;
  /** A setting Brigadier may pick for: a switch over the columns, and picking a model turns it off. */
  automatic?: { chosen: boolean; label: string; hint: string; onChoose: () => void; onTurnOff: () => void } | undefined;
  /** Each agent's own default first among its models, by the agent's name: "Claude's default". */
  agentDefault?: ((provider: ProviderKind) => string) | undefined;
  /** Why a row can't be picked ("In the order", a rule's words); it shows greyed with that. */
  greyed?: ((provider: ProviderKind, model: ModelInfo | null) => string | null) | undefined;
  /** A started session on this agent: the footer says what picking another agent's model does. */
  continuesFrom?: ProviderKind | null | undefined;
  /** Closes the picker (Set up opens Settings). */
  onClose?: (() => void) | undefined;
};

/** The picker's Escape: `true` when it only cleared the search, and the picker stays open. */
type EscapeRef = { current: (() => boolean) | null };

const FAVORITES = "favorites";

/** "xhigh" → "Extra high", "medium" → "Medium". */
export function effortLabel(effort: string): string {
  const known: Record<string, string> = {
    none: "None",
    minimal: "Minimal",
    low: "Low",
    medium: "Medium",
    high: "High",
    xhigh: "Extra high",
    max: "Max",
  };
  return known[effort] ?? effort.charAt(0).toUpperCase() + effort.slice(1);
}

/** A row of the models pane: an agent's model, or its own default. */
type Row = { provider: ProviderKind; model: ModelInfo | null };

const sameRow = (row: Row, choice: PickerChoice | null | undefined) =>
  Boolean(choice) && row.provider === choice!.provider && (row.model?.id ?? null) === choice!.model;

const SIDEBAR_ROW =
  "rounded-toolbar-button text-foreground/70 text-label hover:bg-toolbar-hover focus-visible:bg-toolbar-hover aria-selected:bg-toolbar-hover aria-selected:text-foreground flex h-8 w-full shrink-0 cursor-default items-center gap-2 px-2.5 text-start outline-hidden";
const MODEL_ROW =
  "group-hover/row:bg-toolbar-hover focus-visible:bg-toolbar-hover flex h-8 w-full shrink-0 cursor-default items-center gap-2.5 rounded-xl ps-2.5 pe-8 text-start text-sm outline-hidden aria-disabled:group-hover/row:bg-transparent";
/** One level or speed in the footer's segmented rows: the fill is the chosen one, the outline the keyboard. */
const SEGMENT =
  "text-foreground/50 text-label hover:text-foreground focus-visible:ring-foreground/40 focus-visible:ring-1 data-[state=on]:bg-foreground/10 data-[state=on]:text-foreground rounded-capsule flex h-6 shrink-0 cursor-default items-center gap-1 px-2 whitespace-nowrap outline-hidden transition-colors motion-reduce:transition-none aria-disabled:opacity-50 aria-disabled:hover:text-foreground/50";
const FOOTER_ROW = "flex h-11 items-center gap-3 ps-4 pe-2.25";

/** `name` with the part that matches `query` in bold. */
function Matched({ name, query }: { name: string; query: string }) {
  const at = query ? name.toLowerCase().indexOf(query.toLowerCase()) : -1;
  if (at < 0) return <>{name}</>;
  return (
    <>
      {name.slice(0, at)}
      <span className="font-semibold">{name.slice(at, at + query.length)}</span>
      {name.slice(at + query.length)}
    </>
  );
}

/** A list's rows the arrow keys move through. */
const options = (list: HTMLElement | null) => [...(list?.querySelectorAll<HTMLElement>("[role=option], [data-picker-older]") ?? [])];
/** The levels the keys and a press can take. */
const levels = (row: HTMLElement | null) => [...(row?.querySelectorAll<HTMLElement>("[role=radio]:not([aria-disabled=true])") ?? [])];

/** Whether a key types a character into the search. */
// Space stays the focused row's activation.
const typed = (event: KeyboardEvent) =>
  event.key.length === 1 && event.key !== " " && !event.metaKey && !event.ctrlKey && !event.altKey;

/**
 * The picker's open content. It opens on Favorites when the checked model is starred, otherwise on
 * its agent; with nothing checked, on Favorites when there are any, otherwise on the first agent;
 * and always on the agent in use when it is one agent's models or that agent isn't set up. ↑ ↓ move
 * through the sidebar and the models, ← → between them and on to the footer (→ from a model not
 * picked yet picks it), ← → through the footer's levels, ← from the first back to the models and
 * ↑ ↓ between its rows and the models; Enter picks, typing searches and Escape clears the search
 * before the picker closes. The footer follows the model under the keyboard, else the checked one;
 * a level of a model not picked yet picks it at that level.
 */
export function ModelPickerPanel({
  groups,
  checked,
  opensOn,
  locked,
  onPick,
  effort,
  refused,
  fast,
  automatic,
  agentDefault,
  greyed,
  continuesFrom,
  onClose,
  escapeRef,
}: ModelPickerProps & { escapeRef: EscapeRef }) {
  const id = useId();
  const favorites = useApp((s) => s.settings.favoriteModels);
  const isFavorite = (provider: ProviderKind, model: ModelInfo) =>
    favorites.some((entry) => entry.provider === provider && entry.id === model.id);
  const usable = groups.filter((entry) => entry.unavailable === null);
  const favoriteRows: Row[] = usable.flatMap((entry) =>
    entry.models.filter((model) => isFavorite(entry.provider, model)).map((model) => ({ provider: entry.provider, model })),
  );
  const active = groups.find((group) => group.provider === (checked?.provider ?? opensOn)) ?? groups[0];
  const [browsed, setBrowsing] = useState<ProviderKind | typeof FAVORITES>(() => {
    const onFavorites = checked
      ? favorites.some((entry) => entry.provider === checked.provider && entry.id === checked.model)
      : favoriteRows.length > 0;
    return !locked && (active?.unavailable ?? null) === null && onFavorites ? FAVORITES : (active?.provider ?? FAVORITES);
  });
  // Agents still loading when it opened (or one gone since): it shows where it would open now.
  const browsing =
    browsed === FAVORITES || groups.some((group) => group.provider === browsed) ? browsed : (active?.provider ?? FAVORITES);
  const [query, setQuery] = useState("");
  const [older, setOlder] = useState<ReadonlySet<ProviderKind>>(new Set());
  const [inSearch, setInSearch] = useState(false);
  const [underKeys, setUnderKeys] = useState<Row | null>(null);
  // The model just picked: the keyboard goes to its levels once it is the checked one.
  const keysAfterPick = useRef<Row | null>(null);
  const search = useRef<HTMLInputElement>(null);
  const agents = useRef<HTMLDivElement>(null);
  const models = useRef<HTMLDivElement>(null);
  const column = useRef<HTMLDivElement>(null);
  const speed = useRef<HTMLDivElement>(null);
  const footer = useRef<HTMLDivElement>(null);
  const panel = useRef<HTMLDivElement>(null);
  useEffect(() => {
    escapeRef.current = () => {
      if (!query) return false;
      setQuery("");
      search.current?.focus();
      return true;
    };
  });
  // The search takes the keyboard once the picker is open, after its holder's own focus moves
  // (a menu focuses itself on opening); a timer, as a frame never comes in a hidden window. A key
  // pressed as it opened may have moved it within the picker already: there it stays (#598).
  useEffect(() => {
    const timer = setTimeout(() => {
      if (!panel.current?.contains(document.activeElement)) search.current?.focus();
    });
    return () => clearTimeout(timer);
  }, []);

  const searching = query.trim() !== "";
  const needle = query.trim().toLowerCase();
  const group = groups.find((entry) => entry.provider === browsing) ?? null;
  const matches = (entry: ModelGroup) => entry.models.filter((model) => model.displayName.toLowerCase().includes(needle));
  // An older model sits under Older models unless it is picked, a default or starred.
  const pinned = (provider: ProviderKind, model: ModelInfo) =>
    model.isDefault || isFavorite(provider, model) || sameRow({ provider, model }, checked);
  const shownOf = (entry: ModelGroup) =>
    entry.models.filter((model) => !model.legacy || pinned(entry.provider, model) || older.has(entry.provider));
  // Stars first.
  const starredFirst = (entry: ModelGroup) => {
    const shown = shownOf(entry);
    return [...shown.filter((model) => isFavorite(entry.provider, model)), ...shown.filter((model) => !isFavorite(entry.provider, model))];
  };
  const rows: Row[] = searching
    ? usable.flatMap((entry) => matches(entry).map((model) => ({ provider: entry.provider, model })))
    : browsing === FAVORITES
      ? favoriteRows
      : group && group.unavailable === null
        ? [
            ...(agentDefault ? [{ provider: group.provider, model: null }] : []),
            ...starredFirst(group).map((model) => ({ provider: group.provider, model })),
          ]
        : [];
  const olderCount = !searching && group ? group.models.filter((model) => model.legacy && !pinned(group.provider, model)).length : 0;
  const label = (provider: ProviderKind) => groups.find((entry) => entry.provider === provider)?.label ?? provider;
  const reason = (row: Row) => greyed?.(row.provider, row.model) ?? null;
  // While the search has the keyboard, Enter picks the first match: it shows as the one under the keyboard.
  const first = searching ? rows.find((row) => !reason(row)) : undefined;
  const rowId = (row: Row) => `${id}-model-${row.provider}-${row.model?.id ?? "default"}`;

  // The footer's model: the one under the keyboard, else the checked one.
  const checkedModel = checked && groups.find((entry) => entry.provider === checked.provider)?.models.find((model) => model.id === checked.model);
  // A checked id no agent lists now has no row, and no levels to show.
  const checkedRow: Row | null = checked && (checkedModel || checked.model === null) ? { provider: checked.provider, model: checkedModel ?? null } : null;
  const inHand = (inSearch && first) || underKeys || checkedRow;
  const inHandChecked = Boolean(inHand) && sameRow(inHand!, checked);

  const focusModels = () => {
    const all = options(models.current);
    (all.find((option) => option.getAttribute("aria-selected") === "true") ?? all[0])?.focus();
  };
  const focusAgent = () =>
    options(agents.current)
      .find((option) => option.getAttribute("aria-selected") === "true")
      ?.focus();
  // The checked level; with none, the row itself, so no level looks selected and → goes to the
  // first. A model with no levels leaves the keyboard where it is.
  const focusEffort = () => {
    const all = levels(column.current);
    if (all.length === 0) return;
    (all.find((level) => level.getAttribute("aria-checked") === "true") ?? column.current)?.focus();
  };
  const focusSpeed = () => {
    const all = levels(speed.current);
    (all.find((level) => level.getAttribute("aria-checked") === "true") ?? all[0])?.focus();
  };
  // A pick can take a while to come back (a session's setup, a rules file): until then the footer
  // still shows the last model.
  useEffect(() => {
    if (!keysAfterPick.current || !sameRow(keysAfterPick.current, checked)) return;
    keysAfterPick.current = null;
    focusEffort();
  });
  const browse = (next: ProviderKind | typeof FAVORITES) => {
    setQuery("");
    setUnderKeys(null);
    keysAfterPick.current = null;
    setBrowsing(next);
  };
  const pick = (row: Row, also?: PickedWith) => {
    if (reason(row)) return;
    setUnderKeys(null);
    setInSearch(false);
    onPick(row.provider, row.model, also);
    // The one already checked comes back as no change, with nothing to wait for.
    if (!also && sameRow(row, checked)) focusEffort();
    else keysAfterPick.current = also ? null : row;
  };
  // Focus leaving for the footer keeps the model it acts on.
  const keepsInHand = (to: EventTarget | null) => to instanceof Node && Boolean(footer.current?.contains(to));

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    // The panel's keys are its own, also inside a menu.
    if (event.key !== "Escape") event.stopPropagation();
    // A key pressed while a pick comes back keeps the keyboard where the user took it.
    keysAfterPick.current = null;
    const target = event.target as HTMLElement;
    const part = target.closest<HTMLElement>("[data-picker-part]")?.dataset.pickerPart;
    if (part === "search") {
      const input = target as HTMLInputElement;
      if (event.key === "ArrowDown") {
        event.preventDefault();
        focusModels();
      } else if (event.key === "Enter" && first) {
        event.preventDefault();
        pick(first);
      } else if (event.key === "ArrowRight" && input.selectionStart === query.length) {
        event.preventDefault();
        if (first && !sameRow(first, checked)) pick(first);
        else focusEffort();
      } else if (event.key === "ArrowLeft" && !query) {
        event.preventDefault();
        focusAgent();
      }
      return;
    }
    if (part !== "agents" && part !== "models" && part !== "effort" && part !== "speed") return;
    if (typed(event) || event.key === "Backspace") {
      event.preventDefault();
      setQuery((current) => (event.key === "Backspace" ? current.slice(0, -1) : current + event.key));
      search.current?.focus();
      return;
    }
    const inFooter = part === "effort" || part === "speed";
    const list =
      part === "agents" ? options(agents.current) : part === "models" ? options(models.current) : levels(part === "effort" ? column.current : speed.current);
    const at = list.indexOf(target);
    const move = (to: number) => {
      const next = list[Math.max(0, Math.min(list.length - 1, to))];
      next?.focus();
      if (part === "agents" && next?.dataset.agent) browse(next.dataset.agent as ProviderKind);
    };
    switch (event.key) {
      case "ArrowDown":
        event.preventDefault();
        if (part === "effort") focusSpeed();
        else if (part !== "speed") move(at + 1);
        return;
      case "ArrowUp":
        event.preventDefault();
        if (part === "effort") focusModels();
        else if (part === "speed") {
          if (levels(column.current).length > 0) focusEffort();
          else focusModels();
        } else if (part === "models" && at <= 0) search.current?.focus();
        else move(at - 1);
        return;
      case "Home":
        event.preventDefault();
        move(0);
        return;
      case "End":
        event.preventDefault();
        move(list.length - 1);
        return;
      case "ArrowRight":
        event.preventDefault();
        if (part === "agents") focusModels();
        else if (part === "models") {
          if (underKeys && !sameRow(underKeys, checked)) pick(underKeys);
          else focusEffort();
        } else if (inFooter) move(at + 1);
        return;
      case "ArrowLeft":
        event.preventDefault();
        if (part === "models") focusAgent();
        else if (part === "effort" && at <= 0) focusModels();
        else if (inFooter) move(at - 1);
        return;
      case "Enter":
      case " ":
        if (target.dataset.pickerOlder !== undefined || at < 0) return;
        event.preventDefault();
        if (part === "agents") focusModels();
        else target.click();
        return;
    }
  };

  const sidebarRow = ({
    value,
    label: fullName,
    name,
    slot,
    count,
    dim,
    icon,
  }: {
    value: ProviderKind | typeof FAVORITES;
    label: string;
    name: string;
    slot: string;
    count: number | null;
    dim?: boolean;
    icon: ReactNode;
  }) => {
    const selected = browsing === value;
    return (
      <button
        key={value}
        type="button"
        role="option"
        id={`${id}-agent-${value}`}
        data-slot={slot}
        data-agent={value}
        aria-label={fullName}
        aria-selected={selected}
        tabIndex={selected ? 0 : -1}
        onClick={() => {
          browse(value);
          search.current?.focus();
        }}
        className={cn(SIDEBAR_ROW, dim && "opacity-50")}
      >
        {icon}
        <span className="min-w-0 truncate">{name}</span>
        {count !== null && <span className="text-muted-foreground ms-auto shrink-0 text-xs">{count}</span>}
      </button>
    );
  };

  const modelRow = (row: Row) => {
    const rule = reason(row);
    const isChecked = sameRow(row, checked);
    const own = rowId(row);
    const name = row.model ? row.model.displayName : `${agentDefault?.(row.provider)}'s default`;
    const favorite = row.model !== null && isFavorite(row.provider, row.model);
    return (
      <div key={`${row.provider}:${row.model?.id ?? ""}`} className="group/row relative shrink-0">
        <button
          type="button"
          role="option"
          id={own}
          data-slot="model-picker-model"
          data-refused={rule ?? undefined}
          aria-selected={isChecked}
          aria-disabled={rule ? true : undefined}
          title={rule ?? undefined}
          tabIndex={-1}
          onClick={() => pick(row)}
          onFocus={() => {
            if (!rule) setUnderKeys(row);
          }}
          onBlur={(event) => {
            if (!keepsInHand(event.relatedTarget)) setUnderKeys(null);
          }}
          className={cn(MODEL_ROW, inSearch && row === first && "bg-toolbar-hover")}
        >
          {(searching || browsing === FAVORITES) && <ProviderGlyph provider={row.provider} className={cn("size-icon-md shrink-0", rule && "opacity-45")} />}
          <span className={cn("min-w-0 truncate", rule && "opacity-45")}>
            {searching ? <Matched name={name} query={query.trim()} /> : name}
            {(searching || browsing === FAVORITES) && <span className="sr-only">, {label(row.provider)}</span>}
          </span>
          {isChecked && <Check aria-hidden className="size-icon-sm shrink-0" />}
          {rule && <span className="text-muted-foreground ms-auto min-w-0 truncate text-xs">{rule}</span>}
        </button>
        {row.model && !rule && (
          <button
            type="button"
            data-slot="model-picker-star"
            aria-label={favorite ? `Remove ${name} from favorites` : `Add ${name} to favorites`}
            aria-pressed={favorite}
            onClick={() => void setModelFavorite({ provider: row.provider, id: row.model!.id }, !favorite).catch(() => undefined)}
            className={cn(
              "rounded-toolbar-button hover:bg-foreground/10 focus-visible:bg-foreground/10 absolute end-1.5 top-1.5 grid size-5 place-items-center outline-hidden",
              favorite ? "text-warning" : "text-muted-foreground opacity-0 group-hover/row:opacity-100 focus-visible:opacity-100",
            )}
          >
            {favorite ? <StarFilled aria-hidden className="size-icon-xs" /> : <Star aria-hidden className="size-icon-xs" />}
          </button>
        )}
      </div>
    );
  };

  const notSetUp = (entry: ModelGroup) => {
    const missing = entry.unavailable === "not installed";
    const names = new Intl.ListFormat("en-GB").format(shippedModelNames(entry.provider));
    return (
      <div data-slot="model-picker-not-set-up" className="flex flex-col items-center gap-2.5 px-5 py-7 text-center">
        <ProviderGlyph provider={entry.provider} tile="md" idle />
        <p className="text-foreground text-sm font-medium">
          {entry.label} {missing ? "isn't installed" : "isn't signed in"}
        </p>
        <p className="text-muted-foreground max-w-2xs text-xs">
          {missing ? "Install it and its models show here" : "Sign in and its models show here"}
          {names ? `: ${names}.` : "."}
        </p>
        <Button
          variant="soft"
          size="sm"
          onClick={() => {
            onClose?.();
            openSettings("providers");
          }}
        >
          Set up {entry.label}
        </Button>
      </div>
    );
  };

  const chooseLevel = (level: string) => {
    if (inHand && !inHandChecked) pick(inHand, { effort: level });
    else effort?.onChange(level);
  };
  const auto = automatic?.chosen === true;
  // An agent's own default row has the levels of the model it runs.
  const levelModel =
    inHand?.model ?? (inHand && agentDefault ? groups.find((entry) => entry.provider === inHand.provider)?.models.find((model) => model.isDefault) : null);
  const levelsOf = auto ? [] : (levelModel?.efforts ?? []);
  const ruleOn = (level: string) => (inHand && levelModel ? (refused?.(inHand.provider, levelModel, level) ?? null) : null);
  const levelRule = levelsOf.map(ruleOn).find(Boolean);
  const columnName = auto ? automatic?.label : inHand?.model?.displayName ?? (inHand && agentDefault ? `${agentDefault(inHand.provider)}'s default` : null);
  const noLevels =
    !auto && inHand && !inHand.model && agentDefault
      ? `${agentDefault(inHand.provider)} picks`
      : auto || inHand
        ? "No effort levels"
        : "Pick a model";
  const hasFast = !auto && Boolean(fast) && Boolean(inHand?.model?.fast);
  const fastOn = inHandChecked && fast?.on === true;
  const note =
    continuesFrom && checked && checked.provider !== continuesFrom
      ? `Continues on ${label(checked.provider)}: fresh context, with a hand-off note`
      : continuesFrom && !searching && browsing !== FAVORITES && browsing !== continuesFrom && group?.unavailable === null
        ? `Picking a model continues on ${group.label}: fresh context, with a hand-off note`
        : null;

  return (
    // oxlint-disable-next-line jsx-a11y/no-static-element-interactions -- the keys of the search and the lists inside it
    <div
      ref={panel}
      data-slot="model-picker"
      className="w-model-picker h-model-picker-height flex max-h-(--radix-popover-content-available-height) max-w-full flex-col overflow-hidden"
      onKeyDown={onKeyDown}
    >
      <label data-picker-part="search" className="border-divider flex h-11 shrink-0 items-center gap-2 border-b px-4">
        <MagnifyingGlassSmSearch aria-hidden className="text-muted-foreground size-icon-md shrink-0" />
        <input
          ref={search}
          type="search"
          {...PLAIN_FIELD}
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          placeholder="Search every agent"
          aria-label="Search every agent"
          aria-controls={`${id}-models`}
          aria-activedescendant={inSearch && first ? rowId(first) : undefined}
          onFocus={() => setInSearch(true)}
          onBlur={(event) => {
            if (!keepsInHand(event.relatedTarget)) setInSearch(false);
          }}
          className="placeholder:text-muted-foreground min-w-0 flex-1 bg-transparent text-sm outline-hidden [&::-webkit-search-cancel-button]:hidden"
        />
      </label>
      {automatic && (
        <div data-slot="model-picker-band" className="border-divider flex shrink-0 items-start gap-2.5 border-b px-4 py-3">
          <Sparkles aria-hidden className="text-muted-foreground size-icon-md mt-0.5 shrink-0" />
          <div className="min-w-0 flex-1">
            <div className="text-sm">{automatic.label}</div>
            <div className="text-muted-foreground mt-0.5 text-xs">{automatic.hint}</div>
          </div>
          <Switch
            data-slot="model-picker-automatic"
            aria-label="Automatic"
            checked={auto}
            onCheckedChange={(on) => (on ? automatic.onChoose() : automatic.onTurnOff())}
            className="mt-0.5"
          />
        </div>
      )}
      <div className="flex min-h-0 flex-1">
        {!searching && (
          <div ref={agents} role="listbox" aria-label="Agents" data-picker-part="agents" className={cn("flex w-40 shrink-0 flex-col gap-0.5 overflow-y-auto p-1.5", auto && "opacity-50")}>
            {sidebarRow({
              value: FAVORITES,
              label: "Favorites",
              name: "Favorites",
              slot: "model-picker-favorites",
              count: favoriteRows.length,
              icon: <StarFilled aria-hidden className="text-muted-foreground size-icon-md shrink-0" />,
            })}
            <div role="presentation" className="h-1.5 shrink-0" />
            {groups.map((entry) =>
              sidebarRow({
                value: entry.provider,
                label: entry.label,
                name: cliFacts(entry.provider).vendor,
                slot: "model-picker-agent",
                count: entry.unavailable === null ? entry.models.filter((model) => !model.legacy || pinned(entry.provider, model)).length : null,
                dim: entry.unavailable !== null,
                icon: <ProviderGlyph provider={entry.provider} className="size-icon-md shrink-0" />,
              }),
            )}
          </div>
        )}
        {!searching && <div aria-hidden data-slot="model-picker-divider" className="bg-divider w-px shrink-0" />}
        <div className={cn("scroll-edge-fade flex min-h-0 min-w-0 flex-1 flex-col overflow-y-auto p-1.5", auto && "opacity-50")}>
          <div
            ref={models}
            id={`${id}-models`}
            role="listbox"
            aria-label={searching ? "Matching models" : browsing === FAVORITES ? "Favorites" : `${group?.label ?? ""} models`}
            data-picker-part="models"
            className="flex flex-col gap-0.5"
          >
            {rows.map(modelRow)}
            {olderCount > 0 && (
              <button
                type="button"
                data-picker-older=""
                aria-expanded={older.has(group!.provider)}
                onClick={() =>
                  setOlder((current) => {
                    const next = new Set(current);
                    if (next.has(group!.provider)) next.delete(group!.provider);
                    else next.add(group!.provider);
                    return next;
                  })
                }
                className="text-muted-foreground hover:text-foreground focus-visible:bg-toolbar-hover group flex h-8 shrink-0 items-center gap-2 rounded-xl px-2.5 text-xs outline-hidden"
              >
                <ChevronRight aria-hidden className="size-icon-sm transition-transform group-aria-expanded:rotate-90 motion-reduce:transition-none" />
                Older models · {olderCount}
              </button>
            )}
          </div>
          {searching && rows.length === 0 && (
            <p className="text-muted-foreground px-2.5 py-1.75 text-xs">No models match “{query.trim()}”</p>
          )}
          {!searching && browsing === FAVORITES && rows.length === 0 && (
            <p className="text-muted-foreground px-2.5 py-1.75 text-xs">No favorites yet</p>
          )}
          {!searching && group && group.unavailable !== null && notSetUp(group)}
          {!searching && group && group.unavailable === null && group.models.length === 0 && !agentDefault && (
            <p className="text-muted-foreground px-2.5 py-1.75 text-xs">No models listed yet</p>
          )}
        </div>
      </div>
      <div ref={footer} className={cn("border-divider shrink-0 border-t", auto && "opacity-50")}>
        <div className={FOOTER_ROW}>
          <div className="text-muted-foreground min-w-0 flex-1 text-xs">
            <div className="truncate">
              Effort
              {columnName && <span className="opacity-75"> · {columnName}</span>}
            </div>
            {levelRule && <div className="truncate">{levelRule}</div>}
          </div>
          <div
            ref={column}
            role="radiogroup"
            aria-label="Effort"
            data-slot="model-picker-effort"
            data-picker-part="effort"
            tabIndex={-1}
            className="flex shrink-0 items-center gap-0.5 outline-hidden"
          >
            {levelsOf.length === 0 ? (
              <div className="text-muted-foreground pe-1.75 text-xs">{noLevels}</div>
            ) : (
              levelsOf.map((level) => {
                const rule = ruleOn(level);
                const selected = inHandChecked && effort?.value === level;
                const isDefault = !rule && !selected && levelModel?.defaultEffort === level;
                return (
                  <button
                    type="button"
                    key={level}
                    role="radio"
                    aria-checked={selected}
                    aria-disabled={rule ? true : undefined}
                    data-state={selected ? "on" : "off"}
                    title={rule ?? (isDefault ? "Default" : undefined)}
                    tabIndex={-1}
                    onClick={(event) => {
                      if (rule) return;
                      // The keyboard follows the press, so only the chosen level looks chosen.
                      event.currentTarget.focus();
                      chooseLevel(level);
                    }}
                    className={SEGMENT}
                  >
                    {effortLabel(level)}
                    {rule && <span className="sr-only">{rule}</span>}
                    {isDefault && <span className="sr-only">Default</span>}
                  </button>
                );
              })
            )}
          </div>
        </div>
        {hasFast && (
          <div className={cn(FOOTER_ROW, "h-10 pb-2")}>
            <span className="text-muted-foreground min-w-0 flex-1 truncate text-xs">Speed</span>
            <div ref={speed} role="radiogroup" aria-label="Speed" data-slot="model-picker-speed" data-picker-part="speed" className="flex shrink-0 items-center gap-0.5">
              {(["standard", "fast"] as const).map((choice) => {
                const selected = (choice === "fast") === fastOn;
                return (
                  <button
                    type="button"
                    key={choice}
                    role="radio"
                    aria-checked={selected}
                    data-state={selected ? "on" : "off"}
                    tabIndex={selected ? 0 : -1}
                    onClick={(event) => {
                      event.currentTarget.focus();
                      if (inHandChecked) fast?.onChange(choice === "fast");
                      else if (inHand) pick(inHand, { fast: choice === "fast" });
                    }}
                    className={SEGMENT}
                  >
                    {choice === "fast" && <Bolt aria-hidden className="size-icon-xs" />}
                    {choice === "fast" ? "Fast" : "Standard"}
                  </button>
                );
              })}
            </div>
          </div>
        )}
        {note && (
          <p data-slot="model-picker-continue" className="text-muted-foreground border-divider truncate border-t px-4 py-2 text-xs" title={note}>
            {note}
          </p>
        )}
      </div>
    </div>
  );
}

/**
 * The picker in its popover, opened from `children` (the trigger, as it is). A pick keeps it open.
 * Escape clears the search first; `onEscape` hears the Escape that closes it, before it closes.
 */
export function ModelPicker({
  children,
  open,
  onOpenChange,
  side = "bottom",
  align = "start",
  label,
  onEscape,
  ...panel
}: ModelPickerProps & {
  children: ReactNode;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  side?: "top" | "bottom";
  align?: "start" | "end";
  /** Names the open picker for a screen reader ("Model", "Write with"). */
  label: string;
  onEscape?: (() => void) | undefined;
}) {
  const escape = useRef<() => boolean>(null);
  return (
    <Popover open={open} onOpenChange={onOpenChange}>
      <PopoverTrigger asChild>{children}</PopoverTrigger>
      <PopoverContent
        side={side}
        align={align}
        aria-label={label}
        data-slot="model-picker-popover"
        className="max-w-(--radix-popover-content-available-width) overflow-hidden p-0"
        // The search takes the keyboard (the panel), not the first button in it.
        onOpenAutoFocus={(event) => event.preventDefault()}
        onEscapeKeyDown={(event) => {
          if (escape.current?.()) event.preventDefault();
          else onEscape?.();
        }}
        // Closed, it gives the keyboard back to its trigger, a moment later: not when a picker
        // opened since has it by then (#598).
        onCloseAutoFocus={(event) => {
          if (document.activeElement?.closest("[data-slot=model-picker]")) event.preventDefault();
        }}
      >
        <ModelPickerPanel {...panel} escapeRef={escape} onClose={() => onOpenChange(false)} />
      </PopoverContent>
    </Popover>
  );
}

/** The trigger's keys a menu's trigger takes too: ↓ and ↑ open the picker, as Enter and Space do. */
export function openOnArrows(open: boolean, setOpen: (open: boolean) => void) {
  return (event: KeyboardEvent<HTMLElement>) => {
    if (open || (event.key !== "ArrowDown" && event.key !== "ArrowUp" && event.key !== "Enter")) return;
    event.preventDefault();
    setOpen(true);
  };
}
