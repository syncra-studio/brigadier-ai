import { useMemo } from "react";

import { ImageSquare, Spinner } from "@/components/icons";
import { useBoard } from "@/state/board";
import { type Mockup, mockupOf } from "@/state/mockup";

/** The session's mockup, read from its board. */
export function useMockup(conversationId: string): Mockup | null {
  const tasks = useBoard((s) => (s.board?.conversationId === conversationId ? s.board.tasks : null));
  return useMemo(() => (tasks ? mockupOf(tasks) : null), [tasks]);
}

/** A mockup being built, or built for real. */
export const mockupBusy = (mockup: Mockup | null) => mockup?.status === "building" || mockup?.status === "matching";

/** The Mockup tab's icon: the busy spinner while it is built (or built for real), as a running row shows. */
export function MockupTabIcon({ conversationId }: { conversationId: string }) {
  return mockupBusy(useMockup(conversationId))
    ? <Spinner data-slot="mockup-tab-busy" className="text-muted-foreground size-icon-sm shrink-0 animate-spin motion-reduce:animate-none" />
    : <ImageSquare className="size-icon-sm shrink-0" />;
}
