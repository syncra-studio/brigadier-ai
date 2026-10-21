import { QUIET, PRIMARY } from "@/app/conversation/changesPills";
import type { Merged } from "@/app/conversation/activity/revert";
import { Button } from "@/components/controls";
import { Spinner } from "@/components/icons";

/**
 * The Changes card's footer row that asks before a merge is reverted: what Revert does, then
 * Cancel and Revert; "Reverting…" once sent.
 */
export function RevertConfirmRow({
  merged,
  fileCount,
  counts,
  reverting,
  onCancel,
  onRevert,
}: {
  merged: Merged;
  fileCount: number;
  counts: { insertions: number; deletions: number };
  reverting: boolean;
  onCancel: () => void;
  onRevert: () => void;
}) {
  return (
    <div data-slot="merge-revert-confirm" className="border-divider flex items-center gap-3 border-t pt-3.5 pb-4 ps-13">
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        <span className="text-foreground text-label font-medium">
          {fileCount > 0 ? `Revert ${fileCount === 1 ? "this file" : `these ${fileCount} files`} on ${merged.base}?` : `Revert this merge on ${merged.base}?`}
        </span>
        <span className="text-muted-foreground text-xs">
          {`Adds one commit that undoes ${fileCount > 0 ? `+${counts.insertions} −${counts.deletions}` : "it"}${merged.pr === undefined ? "" : " and pushes it"}. Work merged after this stays.`}
        </span>
      </div>
      {reverting ? (
        <span role="status" className="text-muted-foreground flex shrink-0 items-center gap-1.5 text-label">
          <Spinner className="size-icon-sm animate-spin" />
          Reverting…
        </span>
      ) : (
        <>
          <Button variant="ghost" size="sm" className={QUIET} onClick={onCancel}>Cancel</Button>
          <Button size="sm" className={PRIMARY}
            data-slot="merge-revert-go"
            onClick={onRevert}
          >
            {fileCount === 0 ? "Revert" : fileCount === 1 ? "Revert 1 file" : `Revert ${fileCount} files`}
          </Button>
        </>
      )}
    </div>
  );
}
