import { useContext, useMemo } from "react";

import { RightSidebarContext } from "@/app/conversation/RightSidebar";
import { previewChip } from "@/app/conversation/previewStatus";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { useBoard } from "@/state/board";

/** Reveals the session's running and finished previews in its right panel. */
export function PreviewChip({ conversationId }: { conversationId: string }) {
  const previews = useBoard((s) => (s.board?.conversationId === conversationId ? s.board.previews : null));
  const chip = useMemo(() => (previews ? previewChip(previews) : null), [previews]);
  const rightSidebar = useContext(RightSidebarContext);
  if (!chip) return null;
  return (
    <Button
      variant="ghost"
      size="xs"
      data-slot="preview-chip"
      title={chip.title}
      aria-label={`Show previews: ${chip.label}, ${chip.status}`}
      aria-expanded={Boolean(rightSidebar?.open && rightSidebar.active === "running")}
      onClick={() => rightSidebar?.openTab("running")}
      className={cn("h-pill rounded-capsule flex max-w-56 shrink-0 gap-1.5 px-2.5",
        chip.status === "Running" ? "bg-success/15 text-success" : "bg-muted text-muted-foreground")}
    >
      <span className="min-w-0 truncate">{chip.label}</span>
      <span className="shrink-0">· {chip.status}</span>
    </Button>
  );
}
