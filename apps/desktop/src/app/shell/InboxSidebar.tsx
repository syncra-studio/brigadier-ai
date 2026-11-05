import { useEffect, useMemo, useRef, useState, type RefObject } from "react";

import { underSource } from "@/app/shell/ProjectRow";
import { Archive, ArrowDown, ArrowUp, ArrowUpRight, SidebarError, SidebarQuestion } from "@/components/icons";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/overlays";
import { ConversationRow, EmptyState, NavList, NavSection, RowAction, type RowActions } from "@/components/shared";
import { archiveAll, openSettings } from "@/state/actions";
import { INBOX_SECTIONS, type InboxSection, useInbox } from "@/state/inbox";
import { useApp } from "@/state/store";
import { useSettledTabs, useTabStates } from "@/state/tabStates";

const TITLES: Record<InboxSection, string> = {
  pinned: "Pinned",
  open: "Open",
  archived: "Archived",
};

/** The sidebar's scrolling list, in both modes. */
export const SIDEBAR_LIST = "scroll-edge-fade flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto px-2 pt-1 pb-2";

/** Archived starts folded. */
export const INBOX_FOLDED: InboxSection[] = ["archived"];

/** Of `ids`, those above and below what `list` shows. */
function outOfView(list: HTMLElement, ids: readonly string[]): { above: string[]; below: string[] } {
  const view = list.getBoundingClientRect();
  const above: string[] = [];
  const below: string[] = [];
  for (const id of ids) {
    const row = list.querySelector(`[data-row-id="${CSS.escape(id)}"]`)?.getBoundingClientRect();
    if (!row) continue;
    if (row.top < view.top) above.push(id);
    else if (row.bottom > view.bottom) below.push(id);
  }
  return { above, below };
}

/**
 * "2 need you": every row that waits on the user or failed, while one of them is scrolled out of the
 * list's view: blue if any of them waits, amber when all of them failed. A click brings the next
 * one into view, the nearest below first.
 */
function NeedYouPill({ list, ids, waiting }: { list: RefObject<HTMLDivElement | null>; ids: readonly string[]; waiting: boolean }) {
  const [hidden, setHidden] = useState<{ above: string[]; below: string[] }>({ above: [], below: [] });
  const key = ids.join(" ");
  useEffect(() => {
    const element = list.current;
    if (!element) return;
    const shown = key ? key.split(" ") : [];
    const measure = () => setHidden(outOfView(element, shown));
    measure();
    element.addEventListener("scroll", measure, { passive: true });
    const resized = new ResizeObserver(measure);
    resized.observe(element);
    // A send, pin or archive can move such a row without changing its ID or the viewport.
    const changed = new MutationObserver(measure);
    changed.observe(element, { childList: true, subtree: true, characterData: true });
    return () => {
      element.removeEventListener("scroll", measure);
      resized.disconnect();
      changed.disconnect();
    };
  }, [list, key]);
  const next = hidden.below[0] ?? hidden.above.at(-1);
  if (next === undefined) return null;
  const Arrow = hidden.below.length > 0 ? ArrowDown : ArrowUp;
  return (
    <button
      type="button"
      data-slot="need-you-pill"
      data-tone={waiting ? "waiting" : "failed"}
      onClick={() => {
        list.current
          ?.querySelector(`[data-row-id="${CSS.escape(next)}"]`)
          ?.scrollIntoView({ block: "center", behavior: "smooth" });
      }}
      className="border-border bg-popover text-popover-foreground hover:bg-accent shadow-tooltip absolute start-1/2 bottom-2 flex h-7 -translate-x-1/2 items-center gap-1.5 rounded-full border ps-2 pe-2.5 text-sm whitespace-nowrap"
    >
      {waiting
        ? <SidebarQuestion aria-hidden className="text-link size-icon-md shrink-0" />
        : <SidebarError aria-hidden className="text-warning size-icon-md shrink-0 rotate-180" />}
      {ids.length === 1 ? "1 needs you" : `${ids.length} need you`}
      <Arrow aria-hidden className="text-muted-foreground size-icon-sm shrink-0" />
    </button>
  );
}

/**
 * The sidebar's Inbox mode: one list across all projects, in Pinned and Open, with Archived folded
 * and docked above the footer, its count and the way to Settings › Archived always showing. A row
 * names its project over its title. A row moves only when the user sends it a message, pins,
 * archives or restores it. A section with no rows is left out.
 */
export function InboxSidebar({
  activeId,
  actions,
  isOpen,
  setOpen,
  onShown,
}: {
  activeId: string | null;
  actions: RowActions;
  isOpen: (id: InboxSection) => boolean;
  setOpen: (id: InboxSection, open: boolean) => void;
  /** The rows as they show, top to bottom: what a Shift-click picks a range of. */
  onShown: (ids: string[]) => void;
}) {
  const inbox = useInbox();
  const catalogLoaded = useApp((s) => s.catalogLoaded);
  const shownKey = INBOX_SECTIONS.flatMap((id) => (isOpen(id) ? inbox[id].map((c) => c.id) : [])).join(" ");
  const shown = useMemo(() => (shownKey ? shownKey.split(" ") : []), [shownKey]);
  useEffect(() => onShown(shown), [onShown, shown]);

  const list = useRef<HTMLDivElement>(null);
  const listedIds = (["pinned", "open"] as const).flatMap((id) => (isOpen(id) ? inbox[id].map((c) => c.id) : []));
  const states = useTabStates(listedIds);
  const attention = listedIds.filter((_, index) => states[index] === "needsYou" || states[index] === "failed");
  const mergedKey = useSettledTabs((s) => inbox.open.filter((c) => s.byTab[c.id]?.prMerged).map((c) => c.id).join(" "));
  const merged = mergedKey ? mergedKey.split(" ") : [];

  const empty = INBOX_SECTIONS.every((id) => inbox[id].length === 0);
  const section = (id: InboxSection) =>
    inbox[id].length === 0 ? null : (
      <NavSection
        key={id}
        title={TITLES[id]}
        open={isOpen(id)}
        onOpenChange={(open) => setOpen(id, open)}
        steadyChevron={id === "archived"}
        steadyActions={id === "open" || id === "archived"}
        scrollList={id === "archived"}
        actions={
          id === "archived" ? (
            <>
              <span data-slot="archived-count" className="text-foreground/50 me-1 text-xs tabular-nums">{inbox.archived.length}</span>
              <Tooltip>
                <TooltipTrigger asChild>
                  <RowAction
                    data-slot="open-archived"
                    aria-label="Open Settings › Archived"
                    className="[&_svg]:size-icon-sm"
                    onClick={() => openSettings("archived")}
                  >
                    <ArrowUpRight />
                  </RowAction>
                </TooltipTrigger>
                <TooltipContent side="bottom">Open Settings › Archived</TooltipContent>
              </Tooltip>
            </>
          ) : id === "open" && merged.length > 0 ? (
            <RowAction
              data-slot="archive-merged"
              className="w-auto gap-1.5 pe-0.5 text-xs"
              onClick={() => void archiveAll(merged)}
            >
              <Archive />
              Archive {merged.length} merged
            </RowAction>
          ) : undefined
        }
      >
        <NavList>
          {inbox[id].map((conversation) => (
            <ConversationRow
              key={conversation.id}
              conversation={conversation}
              active={conversation.id === activeId}
              actions={actions}
              forked={underSource(conversation, inbox[id])}
              twoLine={id !== "archived"}
            />
          ))}
        </NavList>
      </NavSection>
    );
  return (
    <>
      <div className="relative flex min-h-0 flex-1 flex-col">
        <div ref={list} className={SIDEBAR_LIST}>
          {section("pinned")}
          {section("open")}
          {catalogLoaded && empty && (
            <NavList>
              <EmptyState as="li" title="Chats and sessions you start appear here." />
            </NavList>
          )}
        </div>
        <NeedYouPill list={list} ids={attention} waiting={states.includes("needsYou")} />
      </div>
      {/* Archived is docked above the footer: open, it grows up to 2/5 of the sidebar and its list scrolls under its title. */}
      {inbox.archived.length > 0 && (
        <div data-slot="inbox-archived" className="flex max-h-2/5 shrink-0 flex-col px-2 pt-1 pb-2">
          {section("archived")}
        </div>
      )}
    </>
  );
}
