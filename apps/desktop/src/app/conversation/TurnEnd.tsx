import { useTurnMerge } from "@/app/conversation/activity/MergeMessage";
import { TurnPreviews } from "@/app/conversation/activity/PreviewCard";
import { webPreviewUrl } from "@/app/conversation/activity/webPreview";
import { isRunRequest } from "@/app/conversation/blocks";
import { ChangesCard } from "@/app/conversation/ChangesCard";
import type { BlockMeta } from "@/app/conversation/RequestBlock";
import { useAuiState } from "@/components/conversation";

/**
 * The end of a finished turn under its answer: its Preview cards (the page it left running and
 * its other previews, a simulator run, a rendered PDF, a test run, each with the proofs taken of
 * it), then its Changes card with the merge that took them (or the merge's alone).
 */
export function TurnEnd({ meta }: { meta: BlockMeta }) {
  const merge = useTurnMerge(meta.orchestratorSteps, meta.requestIds);
  // A run's work is merged from its card, never undone behind the run's back.
  const card = meta.session && !isRunRequest(meta.requestId);
  // The one local address the final answer names, never a late note under it: the page, when a
  // preview serves it.
  const answered = useAuiState((s) => {
    const texts = s.message.parts.filter((candidate) => candidate.type === "text");
    const part = texts[meta.texts.findLastIndex((text) => !text.update)];
    return part?.type === "text" ? webPreviewUrl(part.text) : null;
  });
  return (
    <div data-slot="turn-end" className="mt-activity flex flex-col gap-3 empty:hidden">
      <TurnPreviews requestIds={meta.requestIds} steps={meta.orchestratorSteps} answered={answered} session={meta.session} />
      {/* Changes is always last, right above the answer's time and actions. */}
      {/* A run's merge shows alone: its files are the run's card's. */}
      {(card || merge?.own) && <ChangesCard requestIds={card ? meta.requestIds : []} merge={merge} />}
    </div>
  );
}
