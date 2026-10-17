import { CopyPlan, PlanText, usePlanDoc } from "@/app/conversation/cards/PlanDocument";
import type { PlanDocRef } from "@/state/planDoc";

/** A plan's own tab: the whole plan the user opened from its card, full screen, to read and copy. */
export function PlanTab({ plan }: { plan: PlanDocRef }) {
  const doc = usePlanDoc(plan);
  if (!doc) {
    return <p className="text-muted-foreground px-4 py-3 text-sm">This plan is no longer here.</p>;
  }
  return (
    <div data-slot="plan-tab" className="flex min-h-0 flex-1 flex-col overflow-y-auto">
      <div className="max-w-thread relative mx-auto w-full px-5 pt-6 pb-10">
        <div className="absolute end-3 top-5">
          <CopyPlan markdown={doc.markdown} />
        </div>
        <PlanText markdown={doc.markdown} />
      </div>
    </div>
  );
}
