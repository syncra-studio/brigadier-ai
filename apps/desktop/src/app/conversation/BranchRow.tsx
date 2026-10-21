import { SummaryRow } from "@/components/conversation";

import { Branch } from "@/components/icons";
import type { DiffStat } from "@/ipc/generated";

/** The branch the session's work is on, with its +N −N once known. */
export function BranchRow({ branch, diff }: { branch: string; diff: DiffStat | null }) {
  return (
    <SummaryRow
      data-row="branch"
      title={branch}
      icon={<Branch />}
      meta={
        diff &&
        (diff.insertions > 0 || diff.deletions > 0) && (
          <span>
            <span className="text-success">+{diff.insertions}</span>{" "}
            <span className="text-destructive">−{diff.deletions}</span>
          </span>
        )
      }
    >
      {branch}
    </SummaryRow>
  );
}
