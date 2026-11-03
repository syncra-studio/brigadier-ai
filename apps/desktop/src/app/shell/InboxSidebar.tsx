import { useEffect, useMemo } from "react";

import { underSource } from "@/app/shell/ProjectRow";
import { ConversationRow, EmptyState, NavList, NavSection, type RowActions } from "@/components/shared";
import { INBOX_SECTIONS, type InboxSection, useInbox } from "@/state/inbox";
import { useApp } from "@/state/store";

const TITLES: Record<InboxSection, string> = {
  pinned: "Pinned",
  active: "Active",
  working: "Working",
  snoozed: "Snoozed",
  done: "Done",
};

/** The sidebar's scrolling list, in both modes. */
export const SIDEBAR_LIST = "scroll-edge-fade flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto px-2 pt-1 pb-2";

/** The Working, Snoozed and Done shelves start folded (D7). */
export const INBOX_FOLDED: InboxSection[] = ["working", "snoozed", "done"];

/**
 * The sidebar's Inbox mode (D7): one list across all projects, in Pinned, Active, Working, Snoozed
 * and Done (docked at the bottom); each row names
 * its project after its title. A section with no rows is left out.
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
  const projects = useApp((s) => s.projects);
  const catalogLoaded = useApp((s) => s.catalogLoaded);
  const shownKey = INBOX_SECTIONS.flatMap((id) => (isOpen(id) ? inbox[id].map((c) => c.id) : [])).join(" ");
  const shown = useMemo(() => (shownKey ? shownKey.split(" ") : []), [shownKey]);
  useEffect(() => onShown(shown), [onShown, shown]);

  const empty = INBOX_SECTIONS.every((id) => inbox[id].length === 0);
  const section = (id: InboxSection) =>
    inbox[id].length === 0 ? null : (
      <NavSection key={id} title={TITLES[id]} open={isOpen(id)} onOpenChange={(open) => setOpen(id, open)}>
        <NavList>
          {inbox[id].map((conversation) => (
            <ConversationRow
              key={conversation.id}
              conversation={conversation}
              active={conversation.id === activeId}
              actions={actions}
              forked={underSource(conversation, inbox[id])}
              label={conversation.projectId ? projects[conversation.projectId]?.name : undefined}
            />
          ))}
        </NavList>
      </NavSection>
    );
  return (
    <>
      <div className={SIDEBAR_LIST}>
        {INBOX_SECTIONS.filter((id) => id !== "done").map(section)}
        {catalogLoaded && empty && (
          <NavList>
            <EmptyState as="li" title="Chats and sessions you start appear here." />
          </NavList>
        )}
      </div>
      {/* Done is docked above the footer (D16): open, it grows up to 2/5 of the sidebar and scrolls on its own. */}
      {inbox.done.length > 0 && (
        <div data-slot="inbox-done" className="scroll-edge-fade max-h-2/5 shrink-0 overflow-y-auto px-2 pt-1 pb-2">
          {section("done")}
        </div>
      )}
    </>
  );
}
