import { ImageSquare } from "@openai/apps-sdk-ui/components/Icon";
import { useEffect, useMemo } from "react";

import { Spinner } from "@/components/glyphs/spinner";
import { useBoard } from "@/state/board";
import { type Mockup, mockupOf } from "@/state/mockup";
import { closeMockupTab, showMockupTab } from "@/state/sessionTabs";

function useMockup(conversationId: string): Mockup | null {
  const tasks = useBoard((s) => (s.board?.conversationId === conversationId ? s.board.tasks : null));
  return useMemo(() => (tasks ? mockupOf(tasks) : null), [tasks]);
}

const busy = (mockup: Mockup | null) => mockup?.status === "building" || mockup?.status === "matching";

/**
 * Adds the session's Mockup tab (SESSION-FIXES-PLAN.md Q10) when its mockup starts, when a
 * revision starts, and when it is compared with the live app. Approved and built for real, it
 * closes once instead.
 */
export function useMockupTab(conversationId: string): void {
  const mockup = useMockup(conversationId);
  const stage = mockup?.status === "compared" ? "/compared" : mockup?.status === "matching" ? "/matching" : "";
  const moment = mockup ? `${mockup.task.id}/v${mockup.version}${stage}` : null;
  useEffect(() => {
    if (!moment) return;
    if (moment.endsWith("/matching")) closeMockupTab(conversationId, moment);
    else showMockupTab(conversationId, moment);
  }, [conversationId, moment]);
}

/** The Mockup tab's icon: the busy spinner while it is built (or built for real), as a running row shows. */
export function MockupTabIcon({ conversationId }: { conversationId: string }) {
  return busy(useMockup(conversationId))
    ? <Spinner data-slot="mockup-tab-busy" className="text-muted-foreground size-icon-sm shrink-0 animate-spin motion-reduce:animate-none" />
    : <ImageSquare className="size-icon-sm shrink-0" />;
}

/** Says the Mockup tab is busy to assistive tech, after its title. */
export function MockupTabStatus({ conversationId }: { conversationId: string }) {
  const mockup = useMockup(conversationId);
  return busy(mockup) ? <span className="sr-only">{mockup?.status === "matching" ? ", being built for real" : ", building"}</span> : null;
}
