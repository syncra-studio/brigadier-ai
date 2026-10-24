import {
  forwardRef,
  type KeyboardEvent,
  type ReactNode,
  useEffect,
  useId,
  useImperativeHandle,
  useRef,
  useState,
} from "react";

import { Button } from "@/components/controls/button";
import { Segmented } from "@/components/controls/segmented";
import { TooltipIconButton } from "@/components/controls/tooltip-icon-button";
import { Bolt, Check, ChevronRight, MagnifyingGlassSmSearch, ProviderGlyph, Sparkles } from "@/components/icons";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/overlays/popover";
import type { ModelGroup } from "@/components/shared/model-selector";
import type { ModelInfo, ProviderKind } from "@/ipc/generated";
import { openSettings } from "@/state/actions";
import { cn } from "@/lib/utils";

/*
 * The model picker (Pen: Model picker, S2): agents on the left, the browsed agent's models on the
 * right with one plain line each, a search across every agent on top, and the picked model's
 * effort (and Fast) underneath. Every place that picks a model opens this one panel; each use
 * switches on only the parts it needs.
 */

/** A row the picker checks: an agent's model by id, or (`model` null) the agent's own default. */
export type PickerChoice = { provider: ProviderKind; model: string | null };

export type ModelPickerProps = {
  groups: readonly ModelGroup[];
  /** The row that is checked, if any. */
  checked: PickerChoice | null;
  /** The agent it opens on when no row is checked (the first otherwise). */
  opensOn?: ProviderKind | undefined;
  /** A model picked (`model` null: the agent's default row). */
  onPick: (provider: ProviderKind, model: ModelInfo | null) => void;
  /** The model a new conversation starts on when nothing is picked: marked Default. */
  defaultModel?: PickerChoice | null | undefined;
  /** The checked model's efforts, under the models while its agent is shown. */
  effort?:
    | { levels: readonly string[]; value: string | null; onChange: (level: string) => void; refused?: ((level: string) => string | null) | undefined }
    | undefined;
  /** The checked model's fast tier, a round toggle at the end of the effort line. */
  fast?: { on: boolean; onChange: (on: boolean) => void } | undefined;
  /** "Automatic" above the agents: a setting Brigadier may pick for. */
  automatic?: { chosen: boolean; label: string; hint: string; onChoose: () => void } | undefined;
  /** Each agent's own default first among its models, by the agent's name: "Claude's default · Claude picks". */
  agentDefault?: ((provider: ProviderKind) => string) | undefined;
  /** Why a row can't be picked ("In the order", a rule's words); it shows greyed with that line. */
  greyed?: ((provider: ProviderKind, model: ModelInfo | null) => string | null) | undefined;
  /** A started session on this agent: another agent's models say a pick continues there. */
  continuesFrom?: ProviderKind | null | undefined;
  /** Closes the picker (Set up opens Settings). */
  onClose?: (() => void) | undefined;
};

/** What the panel's holder asks it: Escape clears the search first (`true`: it did). */
export type ModelPickerHandle = { escape: () => boolean };

const AUTOMATIC = "automatic";

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

/** An agent row and a model row: rounded, filled while picked or under the keyboard. */
const AGENT_ROW =
  "flex h-8 w-full shrink-0 cursor-default items-center gap-2.5 rounded-xl px-2.5 text-start text-sm outline-hidden hover:bg-toolbar-hover focus-visible:bg-toolbar-hover aria-selected:bg-toolbar-active";
const MODEL_ROW =
  "flex w-full shrink-0 cursor-default items-center gap-2.5 rounded-xl px-2.5 py-1.75 text-start outline-hidden hover:bg-toolbar-hover focus-visible:bg-toolbar-hover aria-disabled:opacity-45 aria-disabled:hover:bg-transparent";
const SEPARATOR = "bg-foreground/10 my-1 h-px shrink-0";

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

/** The models pane's heading: what it lists, and how many on the right. */
function header(title: string, aside?: string) {
  return (
    <div className="flex items-center gap-2 px-2.5 pt-2 pb-1 text-xs">
      <span className="text-muted-foreground min-w-0 flex-1 truncate font-medium">{title}</span>
      {aside && <span className="text-muted-foreground shrink-0">{aside}</span>}
    </div>
  );
}

const count = (n: number) => `${n} ${n === 1 ? "model" : "models"}`;

/** Whether a key types a character into the search. */
// Space stays the focused row's activation.
const typed = (event: KeyboardEvent) =>
  event.key.length === 1 && event.key !== " " && !event.metaKey && !event.ctrlKey && !event.altKey;

/**
 * The picker's open content. It opens on the checked row's agent (Automatic when that is
 * chosen). ↑ ↓ move through a column, ← → between the two, Enter picks, typing searches and
 * Escape clears the search before the picker closes.
 */
export const ModelPickerPanel = forwardRef<ModelPickerHandle, ModelPickerProps>(function ModelPickerPanel(
  { groups, checked, opensOn, onPick, defaultModel, effort, fast, automatic, agentDefault, greyed, continuesFrom, onClose },
  ref,
) {
  const id = useId();
  const opening = automatic?.chosen
    ? AUTOMATIC
    : (groups.find((group) => group.provider === (checked?.provider ?? opensOn)) ?? groups[0])?.provider ?? AUTOMATIC;
  const [browsed, setBrowsing] = useState<ProviderKind | typeof AUTOMATIC>(opening);
  // Agents still loading when it opened (or one gone since): it shows where it would open now.
  const browsing =
    browsed === AUTOMATIC ? (automatic ? browsed : opening) : groups.some((group) => group.provider === browsed) ? browsed : opening;
  const [query, setQuery] = useState("");
  const [older, setOlder] = useState<ReadonlySet<ProviderKind>>(new Set());
  const [inSearch, setInSearch] = useState(false);
  const search = useRef<HTMLInputElement>(null);
  const agents = useRef<HTMLDivElement>(null);
  const models = useRef<HTMLDivElement>(null);
  useImperativeHandle(ref, () => ({
    escape: () => {
      if (!query) return false;
      setQuery("");
      search.current?.focus();
      return true;
    },
  }));
  // The search takes the keyboard once the picker is open, after its holder's own focus moves
  // (a menu focuses itself on opening); a timer, as a frame never comes in a hidden window.
  useEffect(() => {
    const timer = setTimeout(() => search.current?.focus());
    return () => clearTimeout(timer);
  }, []);

  const searching = query.trim() !== "";
  const needle = query.trim().toLowerCase();
  const group = groups.find((entry) => entry.provider === browsing) ?? null;
  const matches = (entry: ModelGroup) =>
    entry.unavailable === null ? entry.models.filter((model) => model.displayName.toLowerCase().includes(needle)) : [];
  // An older model sits under Older models unless it is picked or a default.
  const pinned = (provider: ProviderKind, model: ModelInfo) =>
    model.isDefault || sameRow({ provider, model }, checked) || sameRow({ provider, model }, defaultModel);
  const rows: Row[] = searching
    ? groups.flatMap((entry) => matches(entry).map((model) => ({ provider: entry.provider, model })))
    : group && group.unavailable === null
      ? [
          ...(agentDefault ? [{ provider: group.provider, model: null }] : []),
          ...group.models
            .filter((model) => !model.legacy || pinned(group.provider, model) || older.has(group.provider))
            .map((model) => ({ provider: group.provider, model })),
        ]
      : [];
  const olderCount = !searching && group ? group.models.filter((model) => model.legacy && !pinned(group.provider, model)).length : 0;
  const label = (provider: ProviderKind) => groups.find((entry) => entry.provider === provider)?.label ?? provider;
  const reason = (row: Row) => greyed?.(row.provider, row.model) ?? null;
  // While the search has the keyboard, Enter picks the first match: it shows as the one under the keyboard.
  const first = searching ? rows.find((row) => !reason(row)) : undefined;
  const rowId = (row: Row) => `${id}-model-${row.provider}-${row.model?.id ?? "default"}`;
  const showsEffort =
    !searching && browsing !== AUTOMATIC && browsing === checked?.provider && (Boolean(effort?.levels.length) || Boolean(fast));

  const focusModels = () => {
    const all = options(models.current);
    (all.find((option) => option.getAttribute("aria-selected") === "true") ?? all[0])?.focus();
  };
  const focusAgent = () =>
    options(agents.current)
      .find((option) => option.getAttribute("aria-selected") === "true")
      ?.focus();
  const browse = (next: ProviderKind | typeof AUTOMATIC) => {
    setQuery("");
    setBrowsing(next);
  };
  const pick = (row: Row) => {
    if (reason(row)) return;
    onPick(row.provider, row.model);
  };

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    // The panel's keys are its own, also inside a menu (a sub-menu's typeahead, its ←).
    if (event.key !== "Escape") event.stopPropagation();
    const target = event.target as HTMLElement;
    const part = target.closest<HTMLElement>("[data-picker-part]")?.dataset.pickerPart;
    if (part === "search") {
      if (event.key === "ArrowDown") {
        event.preventDefault();
        focusModels();
      } else if (event.key === "Enter" && first) {
        event.preventDefault();
        pick(first);
      }
      return;
    }
    if (part !== "agents" && part !== "models") return;
    if (typed(event) || event.key === "Backspace") {
      event.preventDefault();
      setQuery((current) => (event.key === "Backspace" ? current.slice(0, -1) : current + event.key));
      search.current?.focus();
      return;
    }
    const list = options(part === "agents" ? agents.current : models.current);
    const at = list.indexOf(target);
    const move = (to: number) => {
      const next = list[Math.max(0, Math.min(list.length - 1, to))];
      next?.focus();
      if (part === "agents" && next?.dataset.agent) browse(next.dataset.agent as ProviderKind);
    };
    switch (event.key) {
      case "ArrowDown":
        event.preventDefault();
        move(at + 1);
        return;
      case "ArrowUp":
        event.preventDefault();
        if (part === "models" && at <= 0) search.current?.focus();
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
        if (part === "agents") {
          event.preventDefault();
          focusModels();
        }
        return;
      case "ArrowLeft":
        if (part === "models") {
          event.preventDefault();
          focusAgent();
        }
        return;
      case "Enter":
      case " ":
        if (target.dataset.pickerOlder !== undefined) return;
        event.preventDefault();
        if (part === "agents") focusModels();
        else target.click();
        return;
    }
  };

  const agentRow = (provider: ProviderKind | typeof AUTOMATIC) => {
    const entry = groups.find((candidate) => candidate.provider === provider);
    const found = searching && entry ? matches(entry).length : 0;
    const selected = !searching && browsing === provider;
    return (
      <button
        type="button"
        key={provider}
        role="option"
        id={`${id}-agent-${provider}`}
        data-slot="model-picker-agent"
        data-agent={provider}
        aria-selected={selected}
        tabIndex={selected ? 0 : -1}
        onClick={() => browse(provider)}
        className={cn(AGENT_ROW, searching && found === 0 && "opacity-45")}
      >
        {provider === AUTOMATIC ? (
          <Sparkles aria-hidden className="text-muted-foreground size-icon-md shrink-0" />
        ) : (
          <ProviderGlyph provider={provider} className="size-3.5 shrink-0" />
        )}
        <span className={cn("min-w-0 flex-1 truncate", entry?.unavailable && "text-muted-foreground")}>
          {provider === AUTOMATIC ? "Automatic" : entry?.label}
        </span>
        {found > 0 && <span className="text-muted-foreground shrink-0 text-xs">{found}</span>}
      </button>
    );
  };

  const modelRow = (row: Row) => {
    const rule = reason(row);
    const isChecked = sameRow(row, checked);
    const own = rowId(row);
    const name = row.model ? row.model.displayName : `${agentDefault?.(row.provider)}'s default`;
    const line = rule ?? (row.model ? row.model.description : `${agentDefault?.(row.provider)} picks`);
    return (
      <button
        type="button"
        key={`${row.provider}:${row.model?.id ?? ""}`}
        role="option"
        id={own}
        data-slot="model-picker-model"
        data-refused={rule ?? undefined}
        aria-selected={isChecked}
        aria-disabled={rule ? true : undefined}
        aria-describedby={line || searching ? `${own}-line` : undefined}
        tabIndex={-1}
        onClick={() => pick(row)}
        className={cn(MODEL_ROW, inSearch && row === first && "bg-toolbar-hover")}
      >
        {searching && <ProviderGlyph provider={row.provider} className="size-3.5 shrink-0" />}
        <span className="flex min-w-0 flex-1 flex-col gap-0.5">
          <span className="text-foreground truncate text-sm">{searching ? <Matched name={name} query={query.trim()} /> : name}</span>
          {(line || searching) && (
            <span id={`${own}-line`} className="text-muted-foreground truncate text-xs">
              {searching ? [label(row.provider), line].filter(Boolean).join(" · ") : line}
            </span>
          )}
        </span>
        {sameRow(row, defaultModel) && <span className="text-muted-foreground shrink-0 text-xs">Default</span>}
        {isChecked && <Check aria-hidden className="size-icon-md shrink-0" />}
      </button>
    );
  };

  const notSetUp = (entry: ModelGroup) => {
    const missing = entry.unavailable === "not installed";
    return (
      <div data-slot="model-picker-not-set-up" className="flex flex-col items-center gap-2.5 px-5 py-7 text-center">
        <ProviderGlyph provider={entry.provider} tile="md" idle />
        <p className="text-foreground text-sm font-medium">
          {entry.label} {missing ? "isn't installed" : "isn't signed in"}
        </p>
        <p className="text-muted-foreground max-w-2xs text-xs">
          {missing ? "Install it and its models show here." : "Sign in and its models show here."}
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

  return (
    // oxlint-disable-next-line jsx-a11y/no-static-element-interactions -- the keys of the search and the two lists inside it
    <div data-slot="model-picker" className="w-model-picker flex max-w-full flex-col" onKeyDown={onKeyDown}>
      <label data-picker-part="search" className="flex h-9 shrink-0 items-center gap-2 px-2.5">
        <MagnifyingGlassSmSearch aria-hidden className="text-muted-foreground size-icon-md shrink-0" />
        <input
          ref={search}
          type="search"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          placeholder="Search models"
          aria-label="Search models"
          aria-controls={`${id}-models`}
          aria-activedescendant={inSearch && first ? rowId(first) : undefined}
          onFocus={() => setInSearch(true)}
          onBlur={() => setInSearch(false)}
          className="placeholder:text-muted-foreground min-w-0 flex-1 bg-transparent text-sm outline-hidden [&::-webkit-search-cancel-button]:hidden"
        />
      </label>
      <div className={SEPARATOR} />
      <div className="max-h-model-picker-panes flex min-h-0">
        <div
          ref={agents}
          role="listbox"
          aria-label="Agents"
          data-picker-part="agents"
          className="w-model-picker-agents border-divider flex shrink-0 flex-col overflow-y-auto border-e py-1 pe-1"
        >
          {automatic && (
            <>
              {agentRow(AUTOMATIC)}
              <div role="presentation" className={SEPARATOR} />
            </>
          )}
          {groups.map((entry) => agentRow(entry.provider))}
        </div>
        <div className="flex min-w-0 flex-1 flex-col overflow-y-auto py-1 ps-1">
          {!searching && continuesFrom && browsing !== AUTOMATIC && browsing !== continuesFrom && group?.unavailable === null && (
            <p data-slot="model-picker-continue" className="text-muted-foreground px-2.5 pt-2 pb-1.5 text-xs">
              Picking a model continues on {group.label}: fresh context, with a hand-off note.
            </p>
          )}
          {searching ? (
            header(count(rows.length))
          ) : browsing === AUTOMATIC ? (
            header("Automatic")
          ) : group && group.unavailable === null ? (
            header(group.label, count(group.models.length))
          ) : null}
          <div
            ref={models}
            id={`${id}-models`}
            role="listbox"
            aria-label={searching ? "Matching models" : browsing === AUTOMATIC ? "Automatic" : `${group?.label ?? ""} models`}
            data-picker-part="models"
            className="flex flex-col"
          >
            {!searching && browsing === AUTOMATIC && automatic ? (
              <>
                <button
                  type="button"
                  role="option"
                  data-slot="model-picker-automatic"
                  aria-selected={automatic.chosen}
                  tabIndex={-1}
                  onClick={automatic.onChoose}
                  className={MODEL_ROW}
                >
                  <span className="flex min-w-0 flex-1 flex-col gap-0.5">
                    <span className="text-foreground truncate text-sm">{automatic.label}</span>
                    <span className="text-muted-foreground text-xs">{automatic.hint}</span>
                  </span>
                  {automatic.chosen && <Check aria-hidden className="size-icon-md shrink-0" />}
                </button>
                <div className="flex flex-col gap-0.5 px-2.5 py-1.75">
                  <span className="text-muted-foreground text-sm">Pick a model yourself</span>
                  <span className="text-muted-foreground text-xs">Choose an agent on the left</span>
                </div>
              </>
            ) : (
              rows.map(modelRow)
            )}
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
                className="text-muted-foreground hover:text-foreground focus-visible:bg-toolbar-hover group flex shrink-0 items-center gap-2 rounded-xl px-2.5 py-1.75 text-xs outline-hidden"
              >
                <ChevronRight aria-hidden className="size-icon-sm transition-transform group-aria-expanded:rotate-90 motion-reduce:transition-none" />
                Older models · {olderCount}
              </button>
            )}
          </div>
          {searching && rows.length === 0 && (
            <p className="text-muted-foreground px-2.5 py-1.75 text-xs">No models match “{query.trim()}”</p>
          )}
          {!searching && group && group.unavailable !== null && notSetUp(group)}
          {!searching && group && group.unavailable === null && group.models.length === 0 && !agentDefault && (
            <p className="text-muted-foreground px-2.5 py-1.75 text-xs">No models listed yet</p>
          )}
        </div>
      </div>
      {showsEffort && (
        <>
          <div className={SEPARATOR} />
          <div data-slot="model-picker-effort" className="flex flex-col gap-2 px-2 pt-1.5 pb-2">
            {effort && effort.levels.length > 0 && <span className="text-muted-foreground text-xs font-medium">Effort</span>}
            <div className="flex items-center gap-2">
              {effort && effort.levels.length > 0 && (
                <Segmented
                  label="Effort"
                  fill
                  value={effort.value ?? ""}
                  options={effort.levels.map((level) => ({
                    value: level,
                    label: effortLabel(level),
                    reason: effort.refused?.(level) ?? null,
                  }))}
                  onChange={effort.onChange}
                  className="min-w-0 flex-1"
                />
              )}
              {fast && (
                <TooltipIconButton
                  tooltip="Fast"
                  side="top"
                  aria-label="Fast"
                  pressed={fast.on}
                  data-slot="model-picker-fast"
                  onClick={() => fast.onChange(!fast.on)}
                  className="rounded-capsule bg-foreground/5 text-muted-foreground hover:bg-foreground/10 hover:text-foreground aria-pressed:bg-toggle-on aria-pressed:text-foreground aria-pressed:hover:bg-toggle-on ms-auto size-7 shrink-0 active:scale-100"
                >
                  <Bolt className="size-3.5" />
                </TooltipIconButton>
              )}
            </div>
          </div>
        </>
      )}
    </div>
  );
});

/**
 * The picker in its popover, opened from `children` (the trigger, as it is). A pick closes it;
 * Escape clears the search first.
 */
export function ModelPicker({
  children,
  open,
  onOpenChange,
  side = "bottom",
  align = "start",
  label,
  ...panel
}: ModelPickerProps & {
  children: ReactNode;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  side?: "top" | "bottom";
  align?: "start" | "end";
  /** Names the open picker for a screen reader ("Model", "Write with"). */
  label: string;
}) {
  const handle = useRef<ModelPickerHandle>(null);
  return (
    <Popover open={open} onOpenChange={onOpenChange}>
      <PopoverTrigger asChild>{children}</PopoverTrigger>
      <PopoverContent
        side={side}
        align={align}
        aria-label={label}
        data-slot="model-picker-popover"
        className="max-w-(--radix-popover-content-available-width) p-1"
        onEscapeKeyDown={(event) => {
          if (handle.current?.escape()) event.preventDefault();
        }}
      >
        <ModelPickerPanel
          ref={handle}
          {...panel}
          automatic={
            panel.automatic && {
              ...panel.automatic,
              onChoose: () => {
                onOpenChange(false);
                panel.automatic?.onChoose();
              },
            }
          }
          onClose={() => onOpenChange(false)}
          onPick={(provider, model) => {
            onOpenChange(false);
            panel.onPick(provider, model);
          }}
        />
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
