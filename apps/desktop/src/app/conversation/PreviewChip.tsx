import { useMemo } from "react";

import { previewChip } from "@/app/conversation/previewStatus";
import { useAction } from "@/app/conversation/useAction";
import { Button } from "@/components/ui/button";
import { stopPreview } from "@/state/actions";
import { useBoard } from "@/state/board";

/**
 * "web · Running · Stop" in a session's header while the thread keeps something running for the
 * user (a dev server, the app). Stop ends it; the chip goes once nothing runs.
 */
export function PreviewChip({ conversationId }: { conversationId: string }) {
  const previews = useBoard((s) => (s.board?.conversationId === conversationId ? s.board.previews : null));
  const chip = useMemo(() => (previews ? previewChip(previews) : null), [previews]);
  const { busy, error, run } = useAction();
  if (!chip) return null;
  return (
    <span
      data-slot="preview-chip"
      title={error ?? chip.title}
      className="h-pill rounded-capsule bg-success/15 text-success flex max-w-56 shrink-0 items-center gap-1.5 ps-2.5 text-xs font-medium"
    >
      <span aria-hidden className="bg-success size-1.5 shrink-0 rounded-full" />
      <span className="min-w-0 truncate">{chip.label}</span>
      <span className="shrink-0">· Running ·</span>
      <Button
        variant="ghost"
        size="xs"
        disabled={busy}
        onClick={() => run(() => stopPreview(conversationId, chip.stops))}
        className="h-full shrink-0 rounded-capsule px-2 text-current"
      >
        {busy ? "Stopping…" : "Stop"}
      </Button>
    </span>
  );
}
