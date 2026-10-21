import type { ReactNode } from "react";

import { ContextMenu, ContextMenuContent, ContextMenuItem, ContextMenuSeparator, ContextMenuTrigger } from "@/components/overlays";
import { closeSession } from "@/state/actions";
import { CHAT_TAB, closeOtherTabs, closeTab, closeTabsToTheRight, keepTabOpen, type SessionTab, sessionTabs, tabOrder } from "@/state/sessionTabs";

/** A tab's right-click menu: Keep open (a preview), Close, Close others, Close to the right. */
export function TabMenu({
  conversationId,
  tab,
  onRename,
  children,
}: {
  conversationId: string;
  tab: SessionTab | null;
  onRename?: (() => void) | undefined;
  children: ReactNode;
}) {
  const id = tab?.id ?? CHAT_TAB;
  const order = tabOrder(sessionTabs(conversationId));
  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>{children}</ContextMenuTrigger>
      <ContextMenuContent>
        {!tab && (
          <>
            <ContextMenuItem disabled={!onRename} onSelect={() => onRename?.()}>Rename</ContextMenuItem>
            <ContextMenuSeparator />
          </>
        )}
        {tab?.kind === "file" && tab.preview && (
          <>
            <ContextMenuItem onSelect={() => keepTabOpen(conversationId, id)}>
              Keep open
            </ContextMenuItem>
            <ContextMenuSeparator />
          </>
        )}
        <ContextMenuItem onSelect={() => (tab ? closeTab(conversationId, id) : closeSession(conversationId))}>
          Close
        </ContextMenuItem>
        <ContextMenuItem
          disabled={order.length <= (tab ? 2 : 1)}
          onSelect={() => closeOtherTabs(conversationId, id)}
        >
          Close others
        </ContextMenuItem>
        <ContextMenuItem
          disabled={order.indexOf(id) === order.length - 1}
          onSelect={() => closeTabsToTheRight(conversationId, id)}
        >
          Close to the right
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenu>
  );
}
