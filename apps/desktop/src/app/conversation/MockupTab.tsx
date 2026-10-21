import { useEffect } from "react";

import { mockupBusy, useMockup } from "@/components/conversation";
import { closeMockupTab, showMockupTab } from "@/state/sessionTabs";

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

/** Says the Mockup tab is busy to assistive tech, after its title. */
export function MockupTabStatus({ conversationId }: { conversationId: string }) {
  const mockup = useMockup(conversationId);
  return mockupBusy(mockup) ? <span className="sr-only">{mockup?.status === "matching" ? ", being built for real" : ", building"}</span> : null;
}
