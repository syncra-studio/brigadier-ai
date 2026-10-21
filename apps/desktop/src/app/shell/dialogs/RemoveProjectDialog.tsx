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
 * Removes a project from Brigadier. The daemon's preview says what goes: its conversations
 * (archived ones too), their worktrees, the merged branches Brigadier made for them (unmerged
 * ones stay) and, if the user leaves it checked, its Brain. The repository's files never go.
 * Nothing can be removed while one of its conversations works.
 */
export function RemoveProjectDialog({
  project,
  onOpenChange,
}: {
  /** The project to remove; the dialog is open while it is set. */
  project: Project | null;
  onOpenChange: (open: boolean) => void;
}) {
  return (
    <Dialog open={project !== null} onOpenChange={onOpenChange}>
      <DialogContent
        showCloseButton={false}
        className={cn(PROJECT_DIALOG_CARD, "max-w-project-dialog max-h-full gap-7 overflow-y-auto p-6")}
        // The card takes focus, not the close button or an answer.
        onOpenAutoFocus={(event) => {
          event.preventDefault();
          (event.currentTarget as HTMLElement).focus();
        }}
      >
        {project && <RemoveFlow key={project.id} project={project} onOpenChange={onOpenChange} />}
      </DialogContent>
    </Dialog>
  );
}

function RemoveFlow({
  project,
  onOpenChange,
}: {
  project: Project;
  onOpenChange: (open: boolean) => void;
}) {
  const [preview, setPreview] = useState<ProjectRemoval | null>(null);
  const [report, setReport] = useState<RemoveProjectReport | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let current = true;
    previewRemoveProject(project.id).then(
      (removal) => current && setPreview(removal),
      (cause: unknown) => current && setError(errorText(cause)),
    );
    return () => {
      current = false;
    };
  }, [project.id]);

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
