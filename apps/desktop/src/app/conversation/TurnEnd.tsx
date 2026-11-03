import { TurnPreviews } from "@/app/conversation/activity/PreviewCard";
import { webPreviewUrl } from "@/app/conversation/activity/webPreview";
import type { BlockMeta } from "@/app/conversation/RequestBlock";
import { useAuiState } from "@/components/conversation";

/**
 * The end of a finished turn under its answer: its Preview cards (the page it left running and
 * its other previews, a simulator run, a rendered PDF, a test run, each with the proofs taken of
 * it).
 */
export function TurnEnd({ meta }: { meta: BlockMeta }) {
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
    </div>
  );
}
