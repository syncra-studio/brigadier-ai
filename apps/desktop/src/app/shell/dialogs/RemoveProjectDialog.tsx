import { useEffect, useState } from "react";

import { errorText, PROJECT_DIALOG_CARD } from "@/app/shell/dialogs/fields";
import { RemoveForm } from "@/app/shell/dialogs/RemoveForm";
import { Removed } from "@/app/shell/dialogs/Removed";
import { Actions, Head } from "@/app/shell/dialogs/removeProjectParts";
import { Spinner } from "@/components/icons";
import { Button, ErrorLine } from "@/components/controls";
import { Dialog, DialogContent } from "@/components/overlays";
import type { Project, ProjectRemoval, RemoveProjectReport } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { previewRemoveProject } from "@/state/actions";

/**
 * How long the dialog waits for the daemon's preview before it opens on "Looking at…": a quick
 * preview opens straight on the form, so the card doesn't open small and then grow.
 */
const PREVIEW_WAIT_MS = 200;

/** The preview for one project: what it holds, why it couldn't be had, and whether the wait is over. */
type Look = { id: string; preview: ProjectRemoval | null; error: string | null; due: boolean };

/**
 * Removes a project from Brigadier. The daemon's preview says what goes: its conversations
 * (archived ones too), their worktrees, the merged branches Brigadier made for them (unmerged
 * ones stay) and, if the user leaves it checked, its Brain. The repository's files never go.
 * Nothing can be removed while one of its conversations works.
 */
export function RemoveProjectDialog({
  project,
  onOpenChange,
}: {
  /** The project to remove; the dialog is open while it is set (once its preview is in, or after a short wait). */
  project: Project | null;
  onOpenChange: (open: boolean) => void;
}) {
  const id = project?.id ?? null;
  const [look, setLook] = useState<Look | null>(null);

  useEffect(() => {
    if (id === null) return;
    let current = true;
    const update = (change: Partial<Look>) =>
      current &&
      setLook((last) => ({ ...(last?.id === id ? last : { id, preview: null, error: null, due: false }), ...change }));
    const timer = setTimeout(() => update({ due: true }), PREVIEW_WAIT_MS);
    previewRemoveProject(id).then(
      (removal) => update({ preview: removal }),
      (cause: unknown) => update({ error: errorText(cause) }),
    );
    return () => {
      current = false;
      clearTimeout(timer);
      setLook(null);
    };
  }, [id]);

  const shown = look !== null && look.id === id ? look : null;
  const ready = shown !== null && (shown.due || shown.preview !== null || shown.error !== null);
  return (
    <Dialog open={project !== null && ready} onOpenChange={onOpenChange}>
      <DialogContent
        showCloseButton={false}
        className={cn(PROJECT_DIALOG_CARD, "max-w-project-dialog max-h-full gap-7 overflow-y-auto p-6")}
        // The card takes focus, not the close button or an answer.
        onOpenAutoFocus={(event) => {
          event.preventDefault();
          (event.currentTarget as HTMLElement).focus();
        }}
      >
        {project && shown && (
          <RemoveFlow
            key={project.id}
            project={project}
            preview={shown.preview}
            error={shown.error}
            onOpenChange={onOpenChange}
          />
        )}
      </DialogContent>
    </Dialog>
  );
}

function RemoveFlow({
  project,
  preview,
  error,
  onOpenChange,
}: {
  project: Project;
  preview: ProjectRemoval | null;
  error: string | null;
  onOpenChange: (open: boolean) => void;
}) {
  const [report, setReport] = useState<RemoveProjectReport | null>(null);

  if (report) return <Removed name={project.name} report={report} onOpenChange={onOpenChange} />;
  if (!preview) {
    return (
      <>
        <div className="flex flex-col gap-5">
          <Head title={`Remove “${project.name}”?`} beside>
            Looking at what “{project.name}” has…
          </Head>
          {error ? <ErrorLine error={error} className="leading-4.25" /> : <Spinner className="size-icon-sm animate-spin" />}
        </div>
        <Actions>
          <Button type="button" variant="ghost" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
        </Actions>
      </>
    );
  }
  return <RemoveForm preview={preview} onRemoved={setReport} onOpenChange={onOpenChange} />;
}
