import {
  type ReactNode,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";

import { FloatingSummary } from "@/app/conversation/FloatingSummary";
import { JobStack } from "@/app/conversation/JobStack";
import { SummaryContent } from "@/app/conversation/SummaryContent";
import { STACK, useKeptScroll } from "@/app/conversation/summaryScroll";
import { openSummaryByItself, useSummary } from "@/app/conversation/summaryState";
import { Tasks } from "@/components/icons";
import { TooltipIconButton } from "@/components/controls";
import { Popover, PopoverTrigger } from "@/components/overlays";
import type { Conversation } from "@/ipc/generated";
import { tokenPx } from "@/lib/tokens";
import { cn } from "@/lib/utils";
import { setPinnedSummary } from "@/state/actions";
import { summaryShownOf, useApp } from "@/state/store";

// The Sources tab reads these from here.
export { hostOf } from "@/app/conversation/summaryLinks";
export { type UsedSource, useUsedSources } from "@/app/conversation/SummaryContent";

/**
 * Where the thread's pane puts the pinned summary, by the room either side of the thread's
 * column: beside the column, beside it with the column moved aside (and narrowed where it must)
 * to leave the card its own gutter, or (a pane too narrow for that) floating over the thread,
 * opened from the top bar.
 */
type SummaryLayout = "beside" | "shift" | "float";

/**
 * The thread's pane, laid out for the pinned summary when `summary` (a session's own view). It
 * follows its width without rendering: the layout goes on the element for CSS (the column's move,
 * the card's visibility), and to the store only when it changes, for the top bar's button. The
 * first layout lands before the pane eases anything, so a thread opens already in place.
 */
export function SummaryPane({
  summary,
  children,
}: {
  summary: boolean;
  children: ReactNode;
}) {
  const pinned = useApp(summaryShownOf);
  const pane = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const element = pane.current;
    if (!summary || !element) return;
    const column = tokenPx("--container-thread");
    const gutterFrom = tokenPx("--spacing-summary-gutter-from");
    const besideFrom = tokenPx("--spacing-summary-beside-from");
    let frame = 0;
    const observer = new ResizeObserver(([entry]) => {
      if (!entry) return;
      const { width } = entry.contentRect;
      const room = (width - column) / 2;
      const layout: SummaryLayout =
        width < gutterFrom ? "float" : room < besideFrom ? "shift" : "beside";
      element.dataset.summary = layout;
      if (!("settled" in element.dataset)) {
        frame = requestAnimationFrame(() => {
          element.dataset.settled = "";
        });
      }
      // A summary shown stays shown as the window changes: floating where it floats, pinned
      // beside the thread elsewhere.
      const state = useSummary.getState();
      if (state.layout === layout && state.measured) return;
      if (state.layout === "float" && state.floating) setPinnedSummary(true);
      const floating = layout === "float" && (state.floating || summaryShownOf(useApp.getState()));
      useSummary.setState({ layout, measured: true, floating });
    });
    observer.observe(element);
    return () => {
      observer.disconnect();
      cancelAnimationFrame(frame);
      delete element.dataset.summary;
      delete element.dataset.settled;
      useSummary.setState({ layout: "beside", measured: false, floating: false });
    };
  }, [summary]);
  return (
    <div
      ref={pane}
      data-pinned={(summary && pinned) || undefined}
      className="group/pane relative min-h-0 flex-1 overflow-x-clip data-pinned:data-[summary=shift]:thread-column:-translate-x-(--spacing-summary-shift) data-pinned:data-[summary=shift]:thread-column:max-w-thread-beside-summary motion-safe:data-settled:thread-column:transition-[translate] motion-safe:data-settled:thread-column:duration-350 motion-safe:data-settled:thread-column:ease-summary-shift"
    >
      {children}
    </div>
  );
}

/**
 * A session's summary, pinned at the top end of its thread's pane, in the reference order: the
 * project, what the branch changed, what waits on the user, the workers, what runs in the
 * background and the sources; under it the job at work and what waits on a set time (Q88), then a
 * run's own card. It eases in from the pane's end when pinned and out when unpinned. Where the
 * pane has too little room beside the thread's column it hides, and the same cards float over the
 * thread instead, opened from the top bar. While the context card is hidden, the job's cards
 * stay, in its place.
 */
export function PinnedSummary({
  conversation,
}: {
  conversation: Conversation | null;
}) {
  const shown = useApp(summaryShownOf);
  const float = useSummary((s) => s.layout === "float");
  const visible = shown && !float;
  const floating = useSummary((s) => s.floating);
  // A session's card opens by itself once it has its project and branch to show, never to say
  // there is nothing here yet.
  const measured = useSummary((s) => s.measured);
  const ready = conversation?.kind === "session" && conversation.setup?.type === "session" ? conversation.id : null;
  useEffect(() => {
    if (ready && measured) openSummaryByItself(ready);
  }, [ready, measured]);
  // The cards stay for their easing in and out; what they show is kept only while they can be seen.
  const [content, setContent] = useState(visible);
  if (visible && !content) setContent(true);
  const { ref: stack, onScroll } = useKeptScroll(`${conversation?.id ?? "draft"}/column`, visible && content);
  useEffect(() => {
    if (visible || !content) return;
    let current = true;
    // Reading the column's animations applies the hiding first, so its easing out is among them.
    const easing = stack.current?.getAnimations() ?? [];
    void Promise.allSettled(easing.map((animation) => animation.finished)).then(
      () => current && setContent(false),
    );
    return () => {
      current = false;
    };
  }, [visible, content, stack]);

  return (
    <>
      {/* Over the thread's top fade (z-10), which comes after it. */}
      <div className="pointer-events-none absolute inset-y-0 end-0 z-20 flex max-w-full flex-col">
        <aside
          ref={stack}
          aria-label="Context"
          aria-hidden={!visible || undefined}
          data-state={shown ? "open" : "closed"}
          onScroll={onScroll}
          className={cn(
            STACK,
            "summary-hidden:invisible summary-hidden:translate-x-full summary-hidden:scale-80 summary-hidden:opacity-0 origin-top-right motion-safe:group-data-settled/pane:transition-[opacity,translate,scale,visibility] motion-safe:group-data-settled/pane:duration-300 motion-safe:group-data-settled/pane:ease-summary-card",
          )}
        >
          {content && <SummaryContent conversation={conversation} active={visible} />}
        </aside>
      </div>
      {conversation?.kind === "session" && !visible && !(float && floating) && (
        <div className="pointer-events-none absolute inset-y-0 end-0 z-20 flex max-w-full flex-col">
          <div aria-label="Job" data-slot="job-stack" className={STACK}>
            <JobStack conversationId={conversation.id} />
          </div>
        </div>
      )}
      {float && <FloatingSummary conversation={conversation} />}
    </>
  );
}

/**
 * The summary's popover where it floats, around the top bar (its button) and the pane (where it
 * opens). Elsewhere it stays closed and holds nothing.
 */
export function SummaryFloat({ children }: { children: ReactNode }) {
  const floating = useSummary((s) => s.floating);
  return (
    <Popover
      open={floating}
      onOpenChange={(open) => {
        // The user's show or hide holds for every window size.
        setPinnedSummary(open);
        useSummary.setState({ floating: open });
      }}
    >
      {children}
    </Popover>
  );
}

/**
 * The top bar's summary button. Where the pane keeps the summary beside the thread it pins and
 * unpins it; where the summary floats, it opens it over the thread, under the top bar.
 */
export function PinnedSummaryToggle() {
  const pinned = useApp(summaryShownOf);
  const float = useSummary((s) => s.layout === "float");
  const floating = useSummary((s) => s.floating);
  if (!float) {
    return (
      <TooltipIconButton look="toolbar"
        tooltip="Context"
        aria-label="Context"
        aria-pressed={pinned}
        onClick={() => setPinnedSummary(!pinned)}
      >
        <Tasks />
      </TooltipIconButton>
    );
  }
  return (
    <PopoverTrigger asChild>
      <TooltipIconButton look="toolbar"
        tooltip="Context"
        aria-label="Context"
        aria-pressed={floating}
      >
        <Tasks />
      </TooltipIconButton>
    </PopoverTrigger>
  );
}
