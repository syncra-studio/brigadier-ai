import { Info } from "@/components/icons";
import { LabeledDivider } from "@/components/shared";

/** A usage limit, which is no error of the turn: a divider says it, with Try again for later. */
export function UsageLimitDivider({ summary, onTryAgain }: { summary: string; onTryAgain: (() => void) | undefined }) {
  return (
    <LabeledDivider data-slot="usage-limit" role="status">
      <Info aria-hidden className="size-icon-sm shrink-0" />
      <span className="min-w-0 text-center">{summary}</span>
      {onTryAgain && (
        <button type="button" onClick={onTryAgain} className="text-link hover:text-link/80 shrink-0">
          Try again
        </button>
      )}
    </LabeledDivider>
  );
}
