import { useAction } from "@/app/conversation/useAction";
import { ErrorLine } from "@/app/dialogs/fields";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import type { Project } from "@/ipc/generated";
import { useRevealed } from "@/lib/splash";
import { useOnboarding } from "@/state/onboarding";
import { useApp } from "@/state/store";
import { nextToAsk, projectFolder, setFolderTrust } from "@/state/trust";

/**
 * "Do you trust this folder?", for each project whose folder has no answer, oldest first: every
 * way of adding a project ends here, as do projects from before the question and a folder
 * changed in a project's settings. One of the two answers must be picked; both are safe.
 */
export function TrustDialog() {
  const catalogLoaded = useApp((s) => s.catalogLoaded);
  const onboarding = useApp((s) => !s.settings.onboarded);
  const smoke = useApp((s) => s.info?.smoke ?? false);
  const reopened = useOnboarding((s) => s.reopened);
  const project = useApp((s) => nextToAsk(Object.values(s.projects)));
  // Not under the startup screen, and after the first-run setup, whose projects it then asks about.
  const revealed = useRevealed();
  const folder = project && projectFolder(project);
  const open = revealed && catalogLoaded && !smoke && !onboarding && !reopened && folder !== null;
  return (
    <Dialog open={open}>
      <DialogContent
        data-testid="trust-dialog"
        showCloseButton={false}
        className="max-w-md"
        onInteractOutside={(event) => event.preventDefault()}
        onEscapeKeyDown={(event) => event.preventDefault()}
      >
        {open && project && folder && (
          <TrustForm key={`${project.id}\n${folder}`} project={project} folder={folder} />
        )}
      </DialogContent>
    </Dialog>
  );
}

/** The question for one project's folder, and its two answers. */
export function TrustForm({ project, folder }: { project: Project; folder: string }) {
  const action = useAction();
  const answer = (trusted: boolean) => action.run(() => setFolderTrust(project.id, folder, trusted));
  return (
    <div className="grid gap-4">
      <DialogHeader>
        <DialogTitle>Do you trust this folder?</DialogTitle>
        <DialogDescription className="text-foreground font-mono text-xs break-all">
          {folder}
        </DialogDescription>
      </DialogHeader>
      <p>
        Agents will read the files in this folder and run commands in it. Only trust folders
        whose contents you know.
      </p>
      <p className="text-muted-foreground text-xs">
        If you don't trust it, every session here asks before doing anything. You can change this
        in the project's settings.
      </p>
      <ErrorLine error={action.error} />
      <DialogFooter>
        <Button
          type="button"
          variant="outline"
          disabled={action.busy}
          onClick={() => answer(false)}
        >
          Don't trust
        </Button>
        <Button type="button" disabled={action.busy} onClick={() => answer(true)}>
          Trust
        </Button>
      </DialogFooter>
    </div>
  );
}
