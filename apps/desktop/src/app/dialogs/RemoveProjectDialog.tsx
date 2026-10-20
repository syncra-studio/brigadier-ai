import { useEffect, useId, useState, type ReactNode } from "react";

import { ErrorLine, errorText, PROJECT_DIALOG_CARD, PROJECT_DIALOG_TITLE } from "@/app/dialogs/fields";
import { Branch, Chats, Folder, Spinner, X } from "@/components/icons";
import { Button, Checkbox } from "@/components/controls";
import { Dialog, DialogClose, DialogContent, DialogDescription, DialogTitle } from "@/components/overlays";
import type { Project, ProjectRemoval, RemovalBranch, RemoveProjectReport } from "@/ipc/generated";
import { formatBytes, shortPath } from "@/lib/format";
import { cn } from "@/lib/utils";
import { previewRemoveProject, removeProject } from "@/state/actions";

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

/**
 * The title with the close button, and the line under it across the dialog; `beside` (while
 * looking, and once removed) keeps the line under the title, beside the close button.
 */
function Head({ title, beside, children }: { title: string; beside?: boolean; children: ReactNode }) {
  const close = (
    <DialogClose asChild>
      <Button type="button" variant="ghost" size="icon-sm" className="text-muted-foreground" aria-label="Close">
        <X />
      </Button>
    </DialogClose>
  );
  if (beside) {
    return (
      <div className="flex items-start gap-3">
        <div className="flex min-w-0 flex-1 flex-col gap-1.5">
          <DialogTitle className={PROJECT_DIALOG_TITLE}>{title}</DialogTitle>
          <DialogDescription>{children}</DialogDescription>
        </div>
        {close}
      </div>
    );
  }
  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-center gap-3">
        <DialogTitle className={cn(PROJECT_DIALOG_TITLE, "min-w-0 flex-1")}>{title}</DialogTitle>
        {close}
      </div>
      <DialogDescription>{children}</DialogDescription>
    </div>
  );
}

function Actions({ children }: { children: ReactNode }) {
  return <div className="flex justify-end gap-2">{children}</div>;
}

/** One row of what goes: its icon, what it is, and a quiet note on the right. */
function SummaryRow({ icon, label, detail }: { icon: ReactNode; label: string; detail: string }) {
  return (
    <li className="flex h-11 items-center gap-3 px-3.5 [&>svg]:size-icon-md [&>svg]:shrink-0 [&>svg]:text-muted-foreground">
      {icon}
      <span className="text-foreground min-w-0 flex-1 truncate">{label}</span>
      <span className="text-muted-foreground text-label shrink-0">{detail}</span>
    </li>
  );
}

function plural(count: number, one: string, many: string): string {
  return `${count} ${count === 1 ? one : many}`;
}

/** The merged branches Brigadier made that go with the project (not one checked out elsewhere). */
function mergedBranches(preview: ProjectRemoval): RemovalBranch[] {
  return preview.branches.filter((branch) => branch.merged && !branch.checkedOut);
}

function RemoveForm({
  preview,
  onRemoved,
  onOpenChange,
}: {
  preview: ProjectRemoval;
  onRemoved: (report: RemoveProjectReport) => void;
  onOpenChange: (open: boolean) => void;
}) {
  const id = useId();
  const [trashBrain, setTrashBrain] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const conversations = preview.conversations;
  const worktreeBytes = preview.worktrees.reduce((sum, w) => sum + w.bytes, 0);
  const merged = mergedBranches(preview);
  const anything = conversations.length > 0 || preview.worktrees.length > 0 || merged.length > 0;

  const confirm = async () => {
    setBusy(true);
    setError(null);
    try {
      const choices = merged.map((b) => ({ repo: b.repo, name: b.name, tip: b.tip }));
      const report = await removeProject(preview.projectId, choices, !trashBrain);
      if (report.failures.length === 0 && report.keptBranches.every((b) => b.merged)) {
        onOpenChange(false);
      } else {
        onRemoved(report);
      }
    } catch (cause) {
      setError(errorText(cause));
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <div className="flex flex-col gap-5">
        <Head title={`Remove “${preview.name}”?`}>
          {preview.repo
            ? `The repository at ${shortPath(preview.repo)} stays exactly as it is.`
            : "Brigadier forgets this project. Its folder and files stay exactly as they are."}
          {anything && " Only what Brigadier made for this project is deleted:"}
        </Head>
        {anything && (
          <ul aria-label="What is deleted" className="bg-foreground/4 rounded-dialog divide-divider flex flex-col divide-y">
            {conversations.length > 0 && (
              <SummaryRow
                icon={<Chats />}
                label={
                  conversations.length === 1
                    ? `1 ${conversations[0]!.kind === "chat" ? "chat" : "session"}`
                    : `${conversations.length} sessions and chats`
                }
                detail="Can't be undone"
              />
            )}
            {preview.worktrees.length > 0 && (
              <SummaryRow
                icon={<Folder />}
                label={plural(preview.worktrees.length, "worktree", "worktrees")}
                detail={formatBytes(worktreeBytes)}
              />
            )}
            {merged.length > 0 && (
              <SummaryRow
                icon={<Branch />}
                label={plural(merged.length, "merged branch", "merged branches")}
                detail="Unmerged ones stay"
              />
            )}
          </ul>
        )}
        {preview.worktrees.length > 0 && (
          <p className="text-muted-foreground text-xs leading-4.25">
            Uncommitted changes in a worktree are saved as a commit on its branch first.
          </p>
        )}
        {preview.brainBytes > 0 && (
          <div className="flex items-center gap-2.5">
            <Checkbox
              id={`${id}-brain`}
              checked={trashBrain}
              onCheckedChange={(state) => setTrashBrain(state === true)}
            />
            <label htmlFor={`${id}-brain`} className="text-foreground text-label min-w-0 flex-1 leading-4">
              Also move its Brain and code index to the Trash
            </label>
            <span className="text-muted-foreground text-label shrink-0 leading-4">{formatBytes(preview.brainBytes)}</span>
          </div>
        )}
        {preview.blocked && <ErrorLine error={preview.blocked} className="leading-4.25" />}
        <ErrorLine error={error} className="leading-4.25" />
      </div>
      <Actions>
        <Button type="button" variant="ghost" disabled={busy} onClick={() => onOpenChange(false)}>
          Cancel
        </Button>
        <Button
          type="button"
          variant="destructive"
          disabled={busy || preview.blocked !== null}
          onClick={() => void confirm()}
        >
          {busy ? "Removing…" : "Remove"}
        </Button>
      </Actions>
    </>
  );
}

function branchKey(branch: RemovalBranch): string {
  return `${branch.repo}\n${branch.name}`;
}

/** What stayed behind: unmerged branches (with the command that deletes each) and failures. */
function Removed({
  name,
  report,
  onOpenChange,
}: {
  name: string;
  report: RemoveProjectReport;
  onOpenChange: (open: boolean) => void;
}) {
  const kept = report.keptBranches.filter((branch) => !branch.merged);
  return (
    <>
      <div className="flex flex-col gap-5">
        <Head title={`“${name}” was removed`} beside>
          {report.brainTrashedBytes > 0
            ? `Its Brain (${formatBytes(report.brainTrashedBytes)}) is in the Trash.`
            : "Its repository was not touched."}
        </Head>
        {kept.length > 0 && (
          <section aria-labelledby="removed-kept" className="flex flex-col gap-1.5">
            <h3 id="removed-kept" className="text-foreground text-label font-medium">
              Branches kept with unmerged work
            </h3>
            <ul className="flex flex-col gap-2 text-sm">
              {kept.map((branch) => (
                <li key={branchKey(branch)} className="flex flex-col gap-0.5">
                  <span className="leading-4.25">{branch.name}</span>
                  <code className="text-muted-foreground text-xs leading-4.25 break-all select-text">{branch.command}</code>
                </li>
              ))}
            </ul>
          </section>
        )}
        {report.failures.length > 0 && (
          <section aria-labelledby="removed-failures" className="flex flex-col gap-1.5">
            <h3 id="removed-failures" className="text-foreground text-label font-medium">
              Not done
            </h3>
            <ul className="text-destructive flex flex-col gap-1 text-sm">
              {report.failures.map((failure) => (
                <li key={failure}>{failure}</li>
              ))}
            </ul>
          </section>
        )}
      </div>
      <Actions>
        <Button type="button" onClick={() => onOpenChange(false)}>
          Done
        </Button>
      </Actions>
    </>
  );
}
