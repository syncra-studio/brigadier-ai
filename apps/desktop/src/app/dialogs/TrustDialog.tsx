import { useAction } from "@/app/conversation/useAction";
import { ErrorLine } from "@/app/dialogs/fields";
import { DialogPill } from "@/app/dialogs/pills";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import type { Project } from "@/ipc/generated";
import { useRevealed } from "@/lib/splash";
import { useOnboarding } from "@/state/onboarding";
import { useApp } from "@/state/store";
import { projectFolder, setFolderTrust, toAsk } from "@/state/trust";

/**
 * "Trust this folder?", for the project the user is in when its folder has no answer: a new
 * project, one from before the question, or a folder changed in its settings, each asked once
 * when first used. One of the two answers must be picked; both are safe.
 */
export function TrustDialog() {
  const catalogLoaded = useApp((s) => s.catalogLoaded);
  const onboarding = useApp((s) => !s.settings.onboarded);
  const smoke = useApp((s) => s.info?.smoke ?? false);
  const reopened = useOnboarding((s) => s.reopened);
  const project = useApp(toAsk);
  // Not under the startup screen, nor during the first-run setup.
  const revealed = useRevealed();
  const folder = project && projectFolder(project);
  const open = revealed && catalogLoaded && !smoke && !onboarding && !reopened && folder !== null;
  return (
    <Dialog open={open}>
      <DialogContent
        data-testid="trust-dialog"
        showCloseButton={false}
        className="bg-settings-surface border-settings-border rounded-design-dialog max-w-trust shadow-design-dialog text-settings-text gap-0 border p-0 ring-0"
        onInteractOutside={(event) => event.preventDefault()}
        onEscapeKeyDown={(event) => event.preventDefault()}
        // The card itself takes focus, not an answer: Enter never trusts a folder by accident.
        onOpenAutoFocus={(event) => {
          event.preventDefault();
          (event.currentTarget as HTMLElement).focus();
        }}
      >
        {open && project && folder && (
          <TrustForm key={`${project.id}\n${folder}`} project={project} folder={folder} />
        )}
      </DialogContent>
    </Dialog>
  );
}

/** The folder's path split at its last part: where it lives, and the folder's own name. */
export function splitFolder(path: string): { parent: string; name: string } {
  const trimmed = path.length > 1 ? path.replace(/\/+$/, "") : path;
  const cut = trimmed.lastIndexOf("/") + 1;
  return { parent: trimmed.slice(0, cut), name: trimmed.slice(cut) };
}

/** The question for one project's folder, and its two answers. */
export function TrustForm({ project, folder }: { project: Project; folder: string }) {
  const action = useAction();
  const answer = (trusted: boolean) => action.run(() => setFolderTrust(project.id, folder, trusted));
  const { parent, name } = splitFolder(folder);
  return (
    <div className="flex flex-col">
      <div className="flex flex-col gap-3 px-6 pt-6">
        <DialogHeader className="gap-1.5">
          <DialogTitle className="text-dialog-title text-settings-text font-medium">Trust this folder?</DialogTitle>
          <p data-slot="trust-path" className="flex min-w-0 font-mono text-settings-meta">
            <span className="text-settings-text-tertiary">{parent}</span>
            <span className="text-settings-text-secondary">{name}</span>
          </p>
        </DialogHeader>
        <DialogDescription className="text-settings-text-secondary text-settings-small leading-[1.5]">
          Agents can edit and run files here without asking, and the folder's own settings can run code.
        </DialogDescription>
        <ErrorLine error={action.error} />
      </div>
      <div className="flex items-center justify-end gap-2 px-6 pt-6 pb-5">
        <DialogPill look="secondary" disabled={action.busy} onClick={() => answer(false)}>
          Don't trust
        </DialogPill>
        <DialogPill look="primary" disabled={action.busy} onClick={() => answer(true)}>
          Trust folder
        </DialogPill>
      </div>
    </div>
  );
}
