import { type FC, useContext, useEffect, useState } from "react";

import { ViewContext } from "@/app/conversation/viewContext";
import { Button } from "@/components/controls";
import { ThreadHistoryContext, wantsEarlier } from "@/components/conversation";
import { Spinner } from "@/components/icons";
import { useApp } from "@/state/store";

/**
 * Above the messages, while older pages wait in the database: they load as the user scrolls up
 * near the top (a spinner while one does, Try again when it failed), or with "Load earlier messages"
 * when nothing scrolls yet.
 */
export const LoadEarlier: FC = () => {
  const { selection } = useContext(ViewContext);
  const conversationId = selection.type === "conversation" ? selection.id : "";
  // The CLI's thread is read whole: older pages come only where the history says so.
  const hasMore = useContext(ThreadHistoryContext)?.hasMore ?? false;
  const loading = useApp((s) => s.threads[conversationId]?.loading ?? false);
  const earlier = useContext(ThreadHistoryContext);
  const [sentinel, setSentinel] = useState<HTMLDivElement | null>(null);
  const loadOlder = earlier?.loadOlder;
  const auto = hasMore && !loading && !earlier?.failed;
  // Scrolled up by the user to within `EARLIER_PX` of the top, the next page loads: the thread
  // keeps what shows where it is. Heard after the thread's own scroller (which captures).
  useEffect(() => {
    const viewport = sentinel?.closest("[data-slot=aui_thread-viewport]");
    if (!auto || !sentinel || !viewport || !loadOlder) return;
    const scrolled = () => {
      if (wantsEarlier(viewport) && viewport.scrollHeight > viewport.clientHeight) loadOlder().catch(() => {});
    };
    viewport.addEventListener("scroll", scrolled, { passive: true });
    return () => viewport.removeEventListener("scroll", scrolled);
  }, [auto, sentinel, loadOlder]);
  if (!earlier || !hasMore || !loadOlder) return null;
  if (loading) {
    return (
      <div data-slot="earlier-loading" role="status" className="flex justify-center py-4">
        <Spinner className="text-muted-foreground size-icon-sm animate-spin" />
        <span className="sr-only">Loading older messages…</span>
      </div>
    );
  }
  if (earlier.failed) {
    return (
      <div data-slot="earlier-failed" className="mb-4 flex items-center justify-center gap-2">
        <span role="alert" className="text-muted-foreground text-sm">
          Couldn't load older messages
        </span>
        <Button variant="secondary" size="sm" onClick={() => loadOlder().catch(() => {})}>
          Try again
        </Button>
      </div>
    );
  }
  return (
    <div ref={setSentinel} data-slot="earlier-load" className="mb-4 flex justify-center">
      <Button variant="ghost" size="sm" onClick={() => loadOlder().catch(() => {})}>
        Load earlier messages
      </Button>
    </div>
  );
};
