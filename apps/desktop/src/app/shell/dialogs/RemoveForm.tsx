import { useId, useState, type ReactNode } from "react";

import { errorText } from "@/app/shell/dialogs/fields";
import { Actions, Head } from "@/app/shell/dialogs/removeProjectParts";
import { Branch, Chats, Folder } from "@/components/icons";
import { Button, Checkbox, ErrorLine } from "@/components/controls";
import type { ProjectRemoval, RemovalBranch, RemoveProjectReport } from "@/ipc/generated";
import { formatBytes, shortPath } from "@/lib/format";
import { removeProject } from "@/state/actions";

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

/**
 * What removing the project deletes (its conversations, worktrees and merged branches), the
 * Brain choice, and Remove.
 */
export function RemoveForm({
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
