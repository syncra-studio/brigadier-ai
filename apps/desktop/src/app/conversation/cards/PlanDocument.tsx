import { Check, Copy, ExpandSm, Lightbulb } from "@openai/apps-sdk-ui/components/Icon";
import { type ComponentProps, type ReactNode, useContext, useLayoutEffect, useRef, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import { SidePanelContext } from "@/app/conversation/SidePanel";
import { MarkdownBlock } from "@/components/assistant-ui/thread";
import { TooltipIconButton } from "@/components/assistant-ui/tooltip-icon-button";
import { useCopyToClipboard } from "@/hooks/use-copy-to-clipboard";
import { cn } from "@/lib/utils";
import { useBoard } from "@/state/board";
import { type PlanDoc, type PlanDocRef, planDoc, showPlanDoc } from "@/state/planDoc";

/**
 * A plan as a document (THREAD-PARITY-PLAN.md §6): a card in the thread with "Plan" and a bulb
 * in its header, Copy and Open on the right, and the start of the plan under it, clipped with a
 * fade. A click opens the whole plan in the side panel's Plan tab.
 */

/** The plan a reference points to, kept stable while its words don't change. */
export function usePlanDoc(ref: PlanDocRef | null): PlanDoc | null {
  const [title, markdown] = useBoard(
    useShallow((s) => {
      const doc = planDoc(s.board, ref);
      return doc ? [doc.title, doc.markdown] : [null, null];
    }),
  );
  return title === null || markdown === null ? null : { title, markdown };
}

/** A plan's text in the plan's own type: a large title, section headings, compact lists. */
export function PlanText({ markdown }: { markdown: string }) {
  return (
    <div
      data-slot="plan-text"
      className={cn(
        "text-plan text-foreground [&_.aui-md-li]:leading-(--text-plan--line-height) [&_.aui-md-p]:leading-(--text-plan--line-height)",
        "[&_.aui-md-h1]:text-plan-title [&_.aui-md-h1]:mb-2",
        "[&_.aui-md-h2]:text-plan-heading [&_.aui-md-h2]:mt-4 [&_.aui-md-h2]:mb-1",
        "[&_.aui-md-h3]:text-plan [&_.aui-md-h3]:mt-3 [&_.aui-md-h3]:mb-1",
        "[&_.aui-md-p]:my-2 [&_.aui-md-ol]:my-1 [&_.aui-md-ul]:my-1 [&_.aui-md-ol>li]:mt-0 [&_.aui-md-ul>li]:mt-0",
      )}
    >
      <MarkdownBlock text={markdown} />
    </div>
  );
}

/** Copies the plan's text, then says so for a moment. */
export function CopyPlan({ markdown }: { markdown: string }) {
  const { isCopied, copyToClipboard } = useCopyToClipboard();
  return (
    <TooltipIconButton
      tooltip={isCopied ? "Copied" : "Copy plan"}
      side="top"
      className="text-foreground/50 hover:text-foreground [&_svg]:size-icon-sm"
      onClick={(event) => {
        event.stopPropagation();
        copyToClipboard(markdown);
      }}
    >
      {isCopied ? <Check /> : <Copy />}
    </TooltipIconButton>
  );
}

/** The card's frame: its header ("Plan" or "Writing plan", with a bulb) and what it holds. */
function PlanFrame({
  label,
  live = false,
  actions,
  children,
  ...props
}: {
  label: string;
  live?: boolean;
  actions?: ReactNode;
  children?: ReactNode;
} & Omit<ComponentProps<"section">, "children">) {
  return (
    <section
      data-slot="plan-card"
      className="border-work-rule bg-foreground/3 rounded-plan flex min-w-0 flex-col border"
      {...props}
    >
      <header className="h-plan-header flex shrink-0 items-center gap-2 ps-3.5 pe-2">
        <Lightbulb aria-hidden className="text-foreground/50 size-icon-md shrink-0" />
        <span className={cn("min-w-0 flex-1 truncate text-sm", live ? "shimmer" : "text-foreground/50")}>
          {label}
        </span>
        {actions && <span className="flex shrink-0 items-center gap-0.5">{actions}</span>}
      </header>
      {children}
    </section>
  );
}

/** "Writing plan", shimmering, while the thread writes its plan, until the whole plan arrives. */
export function WritingPlanCard() {
  return <PlanFrame label="Writing plan" live aria-label="Writing plan" />;
}

/** The start of a plan, clipped; the fade shows only over a plan that goes on past the clip. */
function PlanClip({ markdown }: { markdown: string }) {
  const clip = useRef<HTMLDivElement>(null);
  const [clipped, setClipped] = useState(false);
  useLayoutEffect(() => {
    const element = clip.current;
    const text = element?.firstElementChild;
    if (!element || !text) return undefined;
    const measure = () => setClipped(element.scrollHeight > element.clientHeight + 1);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(text);
    observer.observe(element);
    return () => observer.disconnect();
  }, []);
  return (
    <div ref={clip} data-clipped={clipped || undefined} className="plan-clip data-clipped:plan-clip-fade">
      <PlanText markdown={markdown} />
    </div>
  );
}

/** The plan's card in the thread. */
export function PlanDocCard({ docRef }: { docRef: PlanDocRef }) {
  const conversationId = useBoard((s) => s.board?.conversationId ?? null);
  const doc = usePlanDoc(docRef);
  // A revision the user sent back, which a newer one replaced.
  const earlier = useBoard((s) => {
    const state = docRef.type === "plan" ? s.board?.plans[docRef.id]?.state.type : null;
    return state === "rejected" || state === "superseded";
  });
  const { openTab } = useContext(SidePanelContext);
  if (!doc) return null;
  const open = () => {
    if (!conversationId) return;
    showPlanDoc(conversationId, docRef);
    openTab("plan");
  };
  return (
    <PlanFrame
      label={earlier ? "Earlier plan" : "Plan"}
      aria-label={`Plan: ${doc.title}`}
      actions={
        <>
          <CopyPlan markdown={doc.markdown} />
          <TooltipIconButton
            tooltip="Open in side panel"
            side="top"
            className="text-foreground/50 hover:text-foreground [&_svg]:size-icon-sm"
            onClick={open}
          >
            <ExpandSm />
          </TooltipIconButton>
        </>
      }
    >
      {/* The plan's text holds headings and lists, which a button can't: a div acts as one. */}
      <div
        role="button"
        tabIndex={0}
        aria-label={`Open the plan: ${doc.title}`}
        onClick={open}
        onKeyDown={(event) => {
          if (event.target !== event.currentTarget || (event.key !== "Enter" && event.key !== " ")) return;
          event.preventDefault();
          open();
        }}
        className="focus-visible:ring-ring rounded-b-plan cursor-pointer px-5 pb-4 text-start outline-none focus-visible:ring-2"
      >
        <PlanClip markdown={doc.markdown} />
      </div>
    </PlanFrame>
  );
}
