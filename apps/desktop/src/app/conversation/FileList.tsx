import { DownloadSimple, File, FileCode, FileDocument, FileImage } from "@openai/apps-sdk-ui/components/Icon";
import { useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { useWorkerText } from "@/app/conversation/WorkerChip";
import { TooltipIconButton } from "@/components/assistant-ui/tooltip-icon-button";
import { Button } from "@/components/ui/button";
import { openArtifact, saveArtifact } from "@/ipc/client";
import type { ArtifactRef } from "@/ipc/generated";
import { formatBytes } from "@/lib/format";

/** Whether the in-app viewer can show it (text); everything else opens in its own app. */
export function isText(artifact: ArtifactRef): boolean {
  return artifact.mime.startsWith("text/") || artifact.mime === "application/json";
}

const EXTENSIONS: Record<string, string> = {
  "text/markdown": ".md",
  "text/x-diff": ".diff",
  "text/plain": ".txt",
  "application/json": ".json",
  "image/png": ".png",
  "image/jpeg": ".jpg",
};

/** The name a file is opened and saved under. */
export function fileNameOf(artifact: ArtifactRef): string {
  return artifact.fileName ?? `${artifact.id.slice(0, 12)}${EXTENSIONS[artifact.mime] ?? ""}`;
}

function FileGlyph({ artifact }: { artifact: ArtifactRef }) {
  const className = "text-foreground/50 size-icon-sm shrink-0";
  if (artifact.mime.startsWith("image/")) return <FileImage aria-hidden className={className} />;
  if (artifact.mime === "text/markdown" || artifact.mime === "text/plain") return <FileDocument aria-hidden className={className} />;
  if (isText(artifact)) return <FileCode aria-hidden className={className} />;
  return <File aria-hidden className={className} />;
}

/** One file: its icon, name and size; the row opens it (text in the viewer), and Save keeps a copy. */
function FileRow({ artifact, onView }: { artifact: ArtifactRef; onView: (artifact: ArtifactRef) => void }) {
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
        {shown.map((artifact) => <FileRow key={artifact.id} artifact={artifact} onView={onView} />)}
      </ul>
      {hidden > 0 && (
        <Button size="xs" variant="ghost" className="text-foreground/60 self-start" onClick={() => setAll(true)}>
          Show {hidden} more
        </Button>
      )}
    </div>
  );
}
