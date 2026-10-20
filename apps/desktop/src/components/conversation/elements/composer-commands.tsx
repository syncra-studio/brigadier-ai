import {
  ComposerPrimitive,
  unstable_useTriggerPopoverScopeContext,
  type Unstable_TriggerItem,
} from "@assistant-ui/react";
import {
  type ComponentProps,
  type FC,
  type ReactNode,
  useEffect,
  useMemo,
  useRef,
} from "react";

import { MatchedText } from "@/components/conversation/elements/fuzzy-match";
import { fuzzyMatch, slashTrigger } from "@/components/conversation/elements/menuMatch";
import { floatingMenu } from "@/components/conversation/elements/surfaces";
import { itemBase, MenuRow } from "@/components/overlays/dropdown-menu";
import { cn } from "@/lib/utils";

/** A command of the composer's `/` menu. */
export type ComposerCommand = {
  id: string;
  label: string;
  description?: string;
  /** A heading the command is listed under ("Skills"); ungrouped commands come first. */
  group?: string;
  icon: ReactNode;
  run: () => void;
};

/** assistant-ui's trigger adapter (its type lives in @assistant-ui/core). */
type TriggerAdapter = NonNullable<
  ComponentProps<typeof ComposerPrimitive.Unstable_TriggerPopover>["adapter"]
>;

/**
 * assistant-ui's trigger adapter over the commands, as one filtered list: no drill-down
 * categories, so every command shows at once with its group heading, and typing filters by
 * name (letters in order are enough).
 */
function commandAdapter(commands: readonly ComposerCommand[]): TriggerAdapter {
  const items: Unstable_TriggerItem[] = commands.map((command) => ({
    id: command.id,
    type: "command",
    label: command.label,
    description: command.description,
    metadata: command.group ? { group: command.group } : undefined,
  }));
  return {
    categories: () => [],
    categoryItems: () => [],
    search(query) {
      // Over names and ids: names that start with the query first, then those
      // holding it, then those with its letters in order; alphabetical within each.
      const rankOf = (item: Unstable_TriggerItem) =>
        Math.min(fuzzyMatch(item.label, query)?.rank ?? 3, fuzzyMatch(item.id, query)?.rank ?? 3);
      return items
        .map((item) => ({ item, rank: rankOf(item) }))
        .filter(({ rank }) => rank < 3)
        .toSorted((a, b) => a.rank - b.rank)
        .map(({ item }) => item);
    },
  };
}

/**
 * The Slash commands element (assistant-ui's composer trigger popover), laid out as a
 * menu above the composer: icon, name and a grey description per row, the first row
 * highlighted, Enter runs it and clears the typed command. Render it inside
 * `ComposerPrimitive.Unstable_TriggerPopoverRoot`, next to the composer.
 */
export const ComposerCommands: FC<{ commands: readonly ComposerCommand[] }> = ({ commands }) => {
  const adapter = useMemo(() => commandAdapter(commands), [commands]);
  const byId = useMemo(() => new Map(commands.map((command) => [command.id, command])), [commands]);
  return (
    <ComposerPrimitive.Unstable_TriggerPopover char="/" matcher={slashTrigger} adapter={adapter}>
      <ComposerPrimitive.Unstable_TriggerPopover.Action
        removeOnExecute
        onExecute={(item) => byId.get(item.id)?.run()}
      />
      <ComposerPrimitive.Unstable_TriggerPopoverItems>
        {(matches) => (
          <CommandList>
            {matches.map((item, index) => {
              const command = byId.get(item.id);
              const group = typeof item.metadata?.group === "string" ? item.metadata.group : null;
              const previous = matches[index - 1]?.metadata?.group ?? null;
              return (
                <div key={item.id} className="contents">
                  {group && group !== previous && (
                    <p className="text-muted-foreground px-2 pt-2 pb-1 text-sm">{group}</p>
                  )}
                  <ComposerPrimitive.Unstable_TriggerPopoverItem
                    item={item}
                    index={index}
                    className={cn(itemBase, "data-highlighted:bg-foreground/8 hover:bg-foreground/8 w-full shrink-0 text-start")}
                  >
                    <MenuRow
                      icon={command?.icon ?? null}
                      title={<MatchedText text={item.label} />}
                      detail={item.description}
                    />
                  </ComposerPrimitive.Unstable_TriggerPopoverItem>
                </div>
              );
            })}
            {matches.length === 0 && (
              <p className="text-muted-foreground px-2 py-1 text-sm">No commands</p>
            )}
          </CommandList>
        )}
      </ComposerPrimitive.Unstable_TriggerPopoverItems>
    </ComposerPrimitive.Unstable_TriggerPopover>
  );
};

/** The menu's scrolling panel, keeping the highlighted row in view as the arrows move it. */
function CommandList({ children }: { children: ReactNode }) {
  const list = useRef<HTMLDivElement>(null);
  const { highlightedIndex } = unstable_useTriggerPopoverScopeContext();
  useEffect(() => {
    if (highlightedIndex < 0) return;
    list.current?.querySelector("[data-highlighted]")?.scrollIntoView({ block: "nearest" });
  }, [highlightedIndex]);
  return (
    <div
      ref={list}
      data-slot="composer-commands"
      aria-label="Slash commands"
      className={cn(
        floatingMenu,
        "absolute start-0 bottom-full z-20 mb-2 flex max-h-command-list w-full flex-col overflow-y-auto",
      )}
    >
      {children}
    </div>
  );
}
