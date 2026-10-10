import { CopyPlan, PlanText, usePlanDoc } from "@/app/conversation/cards/PlanDocument";
import { usePlanTab } from "@/state/planDoc";

/** The side panel's Plan tab: the whole plan the user opened from its card, to read and copy. */
export function PlanTab({ conversationId }: { conversationId: string }) {
  const ref = usePlanTab((s) => s.shown[conversationId] ?? null);
  const doc = usePlanDoc(ref);
  if (!doc) {
    return <p className="text-muted-foreground px-4 py-3 text-sm">This plan is no longer here.</p>;
  }
  return (
    <div data-slot="plan-tab" className="flex min-h-0 flex-1 flex-col overflow-y-auto">
      <div className="relative px-5 pt-1 pb-6">
        <div className="absolute end-3 top-1">
          <CopyPlan markdown={doc.markdown} />
        </div>
        <PlanText markdown={doc.markdown} />
      </div>
    </div>
  );
}
