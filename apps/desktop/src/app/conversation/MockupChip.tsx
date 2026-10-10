import { useContext, useMemo } from "react";

import { RightSidebarContext } from "@/app/conversation/RightSidebar";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { useBoard } from "@/state/board";
import { type MockupStatus, mockupOf } from "@/state/mockup";
import { MOCKUP_TAB, selectTab, useSessionTabsOf } from "@/state/sessionTabs";

const WORDS: Record<MockupStatus, string> = {
  building: "Building", ready: "Ready", stopped: "Stopped", matching: "Building for real", compared: "Compared",
};

/** Opens the session's mockup (SESSION-FIXES-PLAN.md Q10): in full view if it is open there, else in the right sidebar. */
export function MockupChip({ conversationId }: { conversationId: string }) {
  const tasks = useBoard((s) => (s.board?.conversationId === conversationId ? s.board.tasks : null));
  const mockup = useMemo(() => (tasks ? mockupOf(tasks) : null), [tasks]);
  const full = useSessionTabsOf(conversationId).tabs.some((tab) => tab.id === MOCKUP_TAB);
  const rightSidebar = useContext(RightSidebarContext);
  if (!mockup) return null;
  const status = `${WORDS[mockup.status]}${mockup.version > 1 ? ` · v${mockup.version}` : ""}`;
  return (
    <Button
      variant="ghost"
      size="xs"
      data-slot="mockup-chip"
      title={mockup.task.title}
      aria-label={`Show the mockup: ${status}`}
      aria-expanded={Boolean(rightSidebar?.open && rightSidebar.active === "mockup")}
      onClick={() => (full ? selectTab(conversationId, MOCKUP_TAB) : rightSidebar?.openTab("mockup"))}
      className={cn("h-pill rounded-capsule flex max-w-56 shrink-0 gap-1.5 px-2.5",
        mockup.status === "building" || mockup.status === "matching" ? "bg-marker/15 text-marker" : "bg-muted text-muted-foreground")}
    >
      <span className="shrink-0">Mockup</span>
      <span className="min-w-0 truncate">· {status}</span>
    </Button>
  );
}
