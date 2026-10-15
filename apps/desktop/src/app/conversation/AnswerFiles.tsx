import { DownloadSimple } from "@openai/apps-sdk-ui/components/Icon";
import { type FC, useMemo } from "react";

import { answerFiles } from "@/app/conversation/answerFileList";
import { fileNameOf } from "@/app/conversation/FileList";
import { FileTypeIcon } from "@/components/assistant-ui/elements/file-type-icon";
import { openArtifact, saveArtifact } from "@/ipc/client";
import type { Task } from "@/ipc/generated";
import { useBoard } from "@/state/board";
import { toast } from "@/state/toasts";

const NONE: Record<string, Task> = {};

function failed(error: unknown): void {
  toast(error instanceof Error ? error.message : String(error), { tone: "error" });
}

/**
 * Under a finished answer: each file its request's workers left in their outputs folders, as a
 * chip that opens it and its "Save…" (BUILD-PLAN A2), so a deliverable has a home without a
 * copy on the Desktop.
 */
export const AnswerFiles: FC<{ requestIds: readonly string[] }> = ({ requestIds }) => {
  const tasks = useBoard((s) => s.board?.tasks ?? NONE);
  const files = useMemo(() => answerFiles(Object.values(tasks), requestIds), [tasks, requestIds]);
  if (files.length === 0) return null;
  return (
    <ul data-slot="answer-files" aria-label="Files from this answer" className="flex flex-wrap gap-2 pt-2">
      {files.map((file) => {
        const name = fileNameOf(file);
        return (
          <li
            key={file.id}
            data-slot="answer-file"
            className="border-foreground/15 rounded-control flex min-w-0 items-center gap-1 border ps-2 pe-1 text-sm"
          >
            <button
              type="button"
              onClick={() => openArtifact(file.id, name).catch(failed)}
              className="text-foreground/80 hover:text-foreground flex min-w-0 items-center gap-1.5"
            >
              <FileTypeIcon name={name} className="size-icon-sm shrink-0" />
              <span className="truncate">{name}</span>
            </button>
            <button
              type="button"
              data-slot="answer-file-save"
              onClick={() => saveArtifact(file.id, name).catch(failed)}
              className="text-link hover:text-link/80 hover:bg-foreground/5 rounded-control flex items-center gap-1 px-1.5"
            >
              <DownloadSimple aria-hidden className="size-icon-sm" />
              Save…
            </button>
          </li>
        );
      })}
    </ul>
  );
};
