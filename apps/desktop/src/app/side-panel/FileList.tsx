import { useState } from "react";

import { WorkerFileRow } from "@/app/side-panel/WorkerFileRow";
import { Button } from "@/components/controls";
import type { ArtifactRef } from "@/ipc/generated";

/** Files shown before "Show N more". */
const SHOWN = 3;

/**
 * Files a worker left in its outputs folder, as one compact list (THREAD-UX-PLAN.md §3.6): the
 * first three, then "Show N more".
 */
export function FileList({
  artifacts,
  onView,
}: {
  artifacts: readonly ArtifactRef[];
  onView: (artifact: ArtifactRef) => void;
}) {
  const [all, setAll] = useState(false);
  const shown = all ? artifacts : artifacts.slice(0, SHOWN);
  const hidden = artifacts.length - shown.length;
  return (
    <div data-slot="file-list" className="-ms-1.5 flex flex-col">
      <ul className="flex flex-col">
        {shown.map((artifact) => <WorkerFileRow key={artifact.id} artifact={artifact} onView={onView} />)}
      </ul>
      {hidden > 0 && (
        <Button size="xs" variant="ghost" className="text-foreground/60 self-start" onClick={() => setAll(true)}>
          Show {hidden} more
        </Button>
      )}
    </div>
  );
}
