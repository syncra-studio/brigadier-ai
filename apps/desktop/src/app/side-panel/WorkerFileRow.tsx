import { useAction } from "@/app/conversation/useAction";
import { useWorkerText } from "@/app/conversation/WorkerChip";
import { FileGlyph } from "@/app/side-panel/FileGlyph";
import { TooltipIconButton } from "@/components/controls";
import { DownloadSimple } from "@/components/icons";
import { openArtifact, saveArtifact } from "@/ipc/client";
import type { ArtifactRef } from "@/ipc/generated";
import { formatBytes } from "@/lib/format";

import { fileNameOf, isText } from "@/app/side-panel/artifactFiles";

/** One file: its icon, name and size; the row opens it (text in the viewer), and Save keeps a copy. */
export function WorkerFileRow({ artifact, onView }: { artifact: ArtifactRef; onView: (artifact: ArtifactRef) => void }) {
  const action = useAction();
  const name = fileNameOf(artifact);
  const title = useWorkerText(artifact.title);
  return (
    <li className="flex min-w-0 flex-col">
      <div className="hover:bg-foreground/5 rounded-control group/file flex min-h-row min-w-0 items-center gap-2 ps-1.5 pe-1 text-sm">
        <FileGlyph artifact={artifact} />
        <button
          type="button"
          title={title}
          onClick={isText(artifact) ? () => onView(artifact) : () => action.run(() => openArtifact(artifact.id, name))}
          className="flex min-w-0 flex-1 items-center gap-2 text-start"
        >
          <span className="text-foreground/80 min-w-0 truncate">{name}</span>
          <span className="text-foreground/50 shrink-0 text-xs tabular-nums">{formatBytes(artifact.bytes)}</span>
        </button>
        <TooltipIconButton
          tooltip="Save"
          disabled={action.busy}
          onClick={() => action.run(() => saveArtifact(artifact.id, name))}
          className="opacity-0 group-hover/file:opacity-100 focus-visible:opacity-100"
        >
          <DownloadSimple />
        </TooltipIconButton>
      </div>
      {action.error && <p role="alert" className="text-destructive ps-7.5 text-xs">{action.error}</p>}
    </li>
  );
}
