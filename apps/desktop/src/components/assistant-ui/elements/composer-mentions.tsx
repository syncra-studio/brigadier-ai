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
import { floatingMenu, mono } from "@/components/assistant-ui/elements/surfaces";
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
 * The Mentions element (assistant-ui's composer trigger popover) as an `@` menu: one
 * list above the composer mixing what can be mentioned, icon, name and grey detail per row,
 * the first row highlighted, filtered as you type. `search` returns the rows for a query; the
 * composer input hears each pick (it becomes a chip there). Render it inside
 * `ComposerPrimitive.Unstable_TriggerPopoverRoot`, next to the composer.
 */
export const ComposerMentions: FC<{
  search: (query: string) => readonly MentionOption[];
  /** A grey line under the rows for this query ("Type to search for files"), if any. */
  hint?: (query: string) => string | null;
  /** Refresh suggestions once each time the menu opens, not while its query changes. */
  onOpen?: () => void;
}> = ({ search, hint, onOpen }) => {
  // The adapter hands assistant-ui the items; the rows' icons and details are looked up here.
  const shown = useRef(new Map<string, MentionOption>());
  const adapter = useMemo<TriggerAdapter>(
    () => ({
      categories: () => [],
      categoryItems: () => [],
      search(query) {
        const options = search(query);
        shown.current = new Map(options.map((option) => [option.item.id, option]));
        return options.map((option) => option.item);
      },
    }),
    [search],
  );
  return (
    <ComposerPrimitive.Unstable_TriggerPopover char="@" adapter={adapter}>
      <ComposerPrimitive.Unstable_TriggerPopover.Directive formatter={formatter} />
      <ComposerPrimitive.Unstable_TriggerPopoverItems>
        {(matches) => (
          <MentionList hint={hint} onOpen={onOpen}>
            {matches.map((item, index) => {
              const option = shown.current.get(item.id);
              return (
                <ComposerPrimitive.Unstable_TriggerPopoverItem
                  key={item.id}
                  item={item}
                  index={index}
                  className="data-highlighted:bg-accent hover:bg-accent rounded-control flex h-row-sm w-full shrink-0 items-center gap-2 px-2 text-start text-sm outline-none"
                >
                  <span className="text-muted-foreground flex size-icon-md shrink-0 items-center justify-center [&_svg]:size-icon-sm">
                    {option?.icon}
                  </span>
                  <span className="shrink-0">
                    <MatchedText text={option?.name ?? item.label} />
                  </span>
                  <span className="text-muted-foreground min-w-0 flex-1 truncate">
                    {option?.detail}
                  </span>
                  {option?.trailing && (
                    <span className={cn(mono, "text-muted-foreground shrink-0")}>
                      {option.trailing}
                    </span>
                  )}
                </ComposerPrimitive.Unstable_TriggerPopoverItem>
              );
            })}
          </MentionList>
        )}
      </ComposerPrimitive.Unstable_TriggerPopoverItems>
    </ComposerPrimitive.Unstable_TriggerPopover>
  );
};

/** The menu's scrolling panel: keeps the highlighted row in view, ends with the hint. */
function MentionList({
  hint,
  onOpen,
  children,
}: {
  hint: ((query: string) => string | null) | undefined;
  onOpen: (() => void) | undefined;
  children: ReactNode[];
}) {
  const list = useRef<HTMLDivElement>(null);
  const onOpenRef = useRef(onOpen);
  useEffect(() => {
    onOpenRef.current = onOpen;
  });
  const { highlightedIndex, query } = unstable_useTriggerPopoverScopeContext();
  useEffect(() => {
    onOpenRef.current?.();
  }, []);
  useEffect(() => {
    if (highlightedIndex < 0) return;
    list.current?.querySelector("[data-highlighted]")?.scrollIntoView({ block: "nearest" });
  }, [highlightedIndex]);
  const note = hint?.(query) ?? (children.length === 0 ? "No results" : null);
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
      {note && <p className="text-muted-foreground px-2 py-1 text-sm">{note}</p>}
    </div>
  );
}
