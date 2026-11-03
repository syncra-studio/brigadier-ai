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

/** The Working, Snoozed and Done shelves start folded (D7). */
export const INBOX_FOLDED: InboxSection[] = ["working", "snoozed", "done"];

/**
 * The sidebar's Inbox mode (D7): one list across all projects, in Pinned, Active, Working, Snoozed
 * and Done; each row names its project after its title. A section with no rows is left out.
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
  return (
    <>
      {INBOX_SECTIONS.map((id) =>
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
        ),
      )}
      {catalogLoaded && empty && (
        <NavList>
          <EmptyState as="li" title="Chats and sessions you start appear here." />
        </NavList>
      )}
    </>
  );
}
