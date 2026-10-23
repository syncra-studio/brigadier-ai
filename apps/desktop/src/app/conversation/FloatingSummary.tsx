import { type ReactNode, useContext, useEffect, useRef } from "react";

import type { PullRequestStatus } from "@/app/conversation/usePullRequest";
import { SummaryContent } from "@/app/conversation/SummaryContent";
import { STACK, useKeptScroll } from "@/app/conversation/summaryScroll";
import { useSummary } from "@/app/conversation/summaryState";
import { RightSidebarContext } from "@/app/side-panel/RightSidebar";
import { CHAT_TAB, useSessionTabsOf } from "@/state/sessionTabs";
import { AgentsPanelContext } from "@/app/conversation/WorkerChip";
import { PopoverAnchor, PopoverContent } from "@/components/overlays";
import type { Conversation } from "@/ipc/generated";
import { cn } from "@/lib/utils";

/**
 * The summary where it floats: the same column of cards over the thread, at the pane's top end
 * under the top bar, reaching at most the window's bottom inset. The top bar's button or Escape
 * closes it; a click elsewhere or a resize leaves it shown.
 */
export function FloatingSummary({ conversation, cards = null, prStatus }: { conversation: Conversation | null; cards?: ReactNode; prStatus: PullRequestStatus }) {
  const { ref, onScroll } = useKeptScroll(`${conversation?.id ?? "draft"}/column`, true);
  // A row that opens the side panel (the workers, a worker) or a tab moves on to it: the summary
  // closes rather than stay where it opened, over the panel, and focus stays off the top bar's
  // button. Coming back to the chat, as the button itself does from another tab, is not moving on.
  const rightSidebar = useContext(RightSidebarContext);
  const { active: mainTab } = useSessionTabsOf(conversation?.id ?? "draft");
  const { panel: worker } = useContext(AgentsPanelContext);
  const panel = `${rightSidebar?.open}/${rightSidebar?.active}/${worker}`;
  const shownPanel = useRef(panel);
  const shownTab = useRef(mainTab);
  const movedOn = useRef(false);
  useEffect(() => {
    const away = panel !== shownPanel.current || (mainTab !== shownTab.current && mainTab !== CHAT_TAB);
    if (away && useSummary.getState().floating) {
      movedOn.current = true;
      useSummary.setState({ floating: false });
    }
    shownPanel.current = panel;
    shownTab.current = mainTab;
  }, [panel, mainTab]);
  return (
    <>
      {/* The pane's whole width, so the card follows the pane's end as the pane narrows (a side
          panel opening) instead of staying where it first opened. */}
      <PopoverAnchor className="pointer-events-none absolute inset-x-0 top-0 h-0" />
      <PopoverContent
        ref={ref}
        side="bottom"
        align="end"
        sideOffset={0}
        aria-label="Context"
        onScroll={onScroll}
        // It stays over the thread while the user reads or types there: only the top bar's
        // button or Escape closes it.
        onInteractOutside={(event) => event.preventDefault()}
        // Focus the summary itself, not its first button (whose tip would open with it).
        onOpenAutoFocus={(event) => {
          event.preventDefault();
          if (event.currentTarget instanceof HTMLElement) event.currentTarget.focus();
        }}
        onCloseAutoFocus={(event) => {
          if (movedOn.current) event.preventDefault();
          movedOn.current = false;
        }}
        className={cn(
          STACK,
          "text-foreground max-w-(--radix-popover-content-available-width) max-h-(--radix-popover-content-available-height) rounded-none bg-transparent shadow-none ring-0",
        )}
      >
        <SummaryContent conversation={conversation} active cards={cards} prStatus={prStatus} />
      </PopoverContent>
    </>
  );
}
