import { useShallow } from "zustand/react/shallow";

import { CopyButton } from "@/components/shared";
import { TooltipIconButton } from "@/components/controls";
import { ExpandSm } from "@/components/icons";
import { PlanClip } from "@/app/conversation/cards/PlanClip";
import { PlanFrame } from "@/app/conversation/cards/PlanFrame";
import { PlanRating } from "@/app/conversation/cards/PlanRating";
import { PlanText } from "@/app/conversation/cards/PlanText";
import { useBoard } from "@/state/board";
import { type PlanDoc, type PlanDocRef, planDoc, usePlanOpens } from "@/state/planDoc";
import { openPlanTab } from "@/state/sessionTabs";

/**
 * A plan as a document (THREAD-PARITY-PLAN.md §6): a card in the thread with "Plan" and a bulb
 * in its header, Copy and Open on the right, and the start of the plan under it, clipped with a
 * fade. A click opens the whole plan in its own tab.
 */

// The side panel's plan tab takes the plan's text from here.
export { PlanText };

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

/** "Writing plan", shimmering, while the thread writes its plan, until the whole plan arrives. */
export function WritingPlanCard() {
  return <PlanFrame label="Writing plan" live aria-label="Writing plan" />;
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
  const opens = usePlanOpens(conversationId);
  if (!doc) return null;
  const open = () => {
    if (conversationId && opens) openPlanTab(conversationId, docRef, doc.title);
  };
  return (
    <PlanFrame
      label={earlier ? "Earlier plan" : "Plan"}
      aria-label={`Plan: ${doc.title}`}
      actions={
        <>
          {conversationId && docRef.type === "plan" && <PlanRating conversationId={conversationId} planId={docRef.id} />}
          <CopyButton text={doc.markdown} tooltip="Copy plan" side="top" className="text-foreground/50 hover:text-foreground [&_svg]:size-icon-sm" onClick={(event) => event.stopPropagation()} />
          {opens && <TooltipIconButton
            tooltip="Open full screen"
            side="top"
            className="text-foreground/50 hover:text-foreground [&_svg]:size-icon-sm"
            onClick={open}
          >
            <ExpandSm />
          </TooltipIconButton>}
        </>
      }
    >
      {/* The plan's text holds headings and lists, which a button can't: a div acts as one. */}
      {opens ? <div
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
      </div> : <div className="px-5 pb-4"><PlanClip markdown={doc.markdown} /></div>}
    </PlanFrame>
  );
}
