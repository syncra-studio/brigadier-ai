import { SummaryRow } from "@/components/conversation";

import { Branch } from "@/components/icons";
import type { DiffStat } from "@/ipc/generated";

/** The branch the session's work is on, with its +N −N once known, then what isn't committed yet. */
export function BranchRow({ branch, diff, uncommitted = 0 }: { branch: string; diff: DiffStat | null; uncommitted?: number }) {
  const changed = diff !== null && (diff.insertions > 0 || diff.deletions > 0);
  return (
    <SummaryRow
      data-row="branch"
      title={branch}
      icon={<Branch />}
      meta={
        (changed || uncommitted > 0) && (
          <span>
            {changed && (
              <>
                <span className="text-success">+{diff.insertions}</span>{" "}
                <span className="text-destructive">−{diff.deletions}</span>
              </>
            )}
            {changed && uncommitted > 0 && " · "}
            {uncommitted > 0 && `${uncommitted} uncommitted`}
          </span>
        )
      }
    >
      {branch}
    </SummaryRow>
  );
}
