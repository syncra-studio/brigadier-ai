import {
  ComposerPrimitive,
  unstable_useTriggerPopoverScopeContext,
  type Unstable_DirectiveFormatter,
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

import { MatchedText } from "@/components/assistant-ui/elements/fuzzy-match";
import { floatingMenu } from "@/components/assistant-ui/elements/surfaces";
import { itemBase, MenuRow } from "@/components/ui/dropdown-menu";
import { cn } from "@/lib/utils";

/** One row of the `@` menu. `item.label` is what the mention leaves in the text, after `@`. */
export type MentionOption = {
  item: Unstable_TriggerItem;
  icon: ReactNode;
  /** The row's name, when it differs from the label (a file's name for its path). */
  name?: string | undefined;
  /** Grey text after the name (a worker's title, a file's folder). */
  detail?: string | undefined;
  /** Grey text at the row's end (a worker's state). */
  trailing?: string | undefined;
};

/** A part of the `@` menu: its heading (none for ranked results), rows and a grey note under them. */
export type MentionGroup = {
  title: string | null;
  options: readonly MentionOption[];
  note?: string | undefined;
};

/** assistant-ui's trigger adapter (its type lives in @assistant-ui/core). */
type TriggerAdapter = NonNullable<
  ComponentProps<typeof ComposerPrimitive.Unstable_TriggerPopover>["adapter"]
>;

/**
 * A mention is plain `@label` text in the message, so it reads naturally everywhere; the
 * composer input's own formatter turns saved text back into chips.
 */
const formatter: Unstable_DirectiveFormatter = {
  serialize: (item) => `@${item.label}`,
  parse: (text) => [{ kind: "text", text }],
};

/**
 * The Mentions element (assistant-ui's composer trigger popover) as an `@` menu above the
 * composer: icon, name and grey detail per row, the first row highlighted. `search` returns
 * the menu's groups for a query, each under its heading with an optional grey note; the
 * composer input hears each pick (it becomes a chip there). Render it inside
 * `ComposerPrimitive.Unstable_TriggerPopoverRoot`, next to the composer.
 */
export const ComposerMentions: FC<{
  search: (query: string) => readonly MentionGroup[];
  /** Refresh suggestions once each time the menu opens, not while its query changes. */
  onOpen?: () => void;
}> = ({ search, onOpen }) => {
  // The adapter hands assistant-ui the items; the groups and the rows' icons and details are
  // looked up here.
  const shown = useRef<readonly MentionGroup[]>([]);
  const adapter = useMemo<TriggerAdapter>(
    () => ({
      categories: () => [],
      categoryItems: () => [],
      search(query) {
        shown.current = search(query);
        return shown.current.flatMap((group) => group.options.map((option) => option.item));
      },
    }),
    [search],
  );
  return (
    <ComposerPrimitive.Unstable_TriggerPopover char="@" adapter={adapter}>
      <ComposerPrimitive.Unstable_TriggerPopover.Directive formatter={formatter} />
      <ComposerPrimitive.Unstable_TriggerPopoverItems>
        {(matches) => {
          const index = new Map(matches.map((item, at) => [item.id, at]));
          const options = new Map(
            shown.current.flatMap((group) => group.options.map((option) => [option.item.id, option] as const)),
          );
          const empty = matches.length === 0 && !shown.current.some((group) => group.note);
          return (
            <MentionList empty={empty} onOpen={onOpen}>
              {shown.current.map((group, at) => {
                const items = matches.filter((item) => group.options.some((option) => option.item.id === item.id));
                if (items.length === 0 && !group.note) return null;
                return (
                  <div key={group.title ?? `results-${at}`} className="contents">
                    {group.title && <p className="text-muted-foreground px-2 pt-2 pb-1 text-sm">{group.title}</p>}
                    {items.map((item) => {
                      const option = options.get(item.id);
                      return (
                        <ComposerPrimitive.Unstable_TriggerPopoverItem
                          key={item.id}
                          item={item}
                          index={index.get(item.id) ?? 0}
                          className={cn(itemBase, "data-highlighted:bg-foreground/8 hover:bg-foreground/8 w-full shrink-0 text-start")}
                        >
                          <MenuRow
                            icon={option?.icon ?? null}
                            title={<MatchedText text={option?.name ?? item.label} />}
                            detail={option?.detail}
                            hint={option?.trailing}
                            mono
                          />
                        </ComposerPrimitive.Unstable_TriggerPopoverItem>
                      );
                    })}
                    {group.note && <p className="text-muted-foreground px-2 py-1 text-sm">{group.note}</p>}
                  </div>
                );
              })}
            </MentionList>
          );
        }}
      </ComposerPrimitive.Unstable_TriggerPopoverItems>
    </ComposerPrimitive.Unstable_TriggerPopover>
  );
};

/** The menu's scrolling panel: keeps the highlighted row in view, says when nothing matches. */
function MentionList({
  empty,
  onOpen,
  children,
}: {
  /** Nothing to show: no row matches and no group has a note. */
  empty: boolean;
  onOpen: (() => void) | undefined;
  children: ReactNode;
}) {
  const list = useRef<HTMLDivElement>(null);
  const onOpenRef = useRef(onOpen);
  useEffect(() => {
    onOpenRef.current = onOpen;
  });
  const { highlightedIndex } = unstable_useTriggerPopoverScopeContext();
  useEffect(() => {
    onOpenRef.current?.();
  }, []);
  useEffect(() => {
    if (highlightedIndex < 0) return;
    list.current?.querySelector("[data-highlighted]")?.scrollIntoView({ block: "nearest" });
  }, [highlightedIndex]);
  return (
    <div
      ref={list}
      data-slot="composer-mentions"
      aria-label="Mentions"
      className={cn(
        floatingMenu,
        "absolute start-0 bottom-full z-20 mb-2 flex max-h-command-list w-full flex-col overflow-y-auto",
      )}
    >
      {children}
      {empty && <p className="text-muted-foreground px-2 py-1 text-sm">No results</p>}
    </div>
  );
}
