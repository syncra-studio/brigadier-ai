import type { FC } from "react";

import type { ConflictFile } from "@/ipc/generated";

/** During a rebase, the commit it replays: "Rebasing 2 of 5: <subject>". */
export const RebaseStep: FC<{ file: ConflictFile }> = ({ file }) => {
  const rebase = file.rebasing;
  if (!rebase) return null;
  const subject = rebase.message.split("\n")[0] ?? "";
  return (
    <div data-slot="conflict-rebase" className="border-border text-muted-foreground flex h-control-sm shrink-0 items-center border-b px-3.5 text-xs">
      <span className="truncate">
        Rebasing {rebase.step} of {rebase.total}
        {subject && <span className="text-foreground">: {subject}</span>}
      </span>
    </div>
  );
};
