import { useAction } from "@/app/conversation/useAction";
import { ErrorLine } from "@/app/dialogs/fields";
import { Button } from "@/components/ui/button";
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
        className="rounded-question max-w-question shadow-menu p-5"
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
    <div className="grid gap-4 text-center">
      <DialogHeader className="gap-4">
        <DialogTitle className="text-xl leading-snug font-semibold">Trust this folder?</DialogTitle>
        <p className="text-muted-foreground break-all select-text">{folder}</p>
        <DialogDescription>
          Agents can read, edit and run the files here, and the folder's own settings can run code
          without being asked. Continue only if you trust these files.
        </DialogDescription>
      </DialogHeader>
      <ErrorLine error={action.error} />
      <div className="grid gap-3 pt-3">
        <Button
          type="button"
          size="lg"
          className="rounded-capsule w-full"
          autoFocus
          disabled={action.busy}
          onClick={() => answer(true)}
        >
          Trust folder
        </Button>
        <Button
          type="button"
          size="lg"
          variant="ghost"
          className="rounded-capsule ring-foreground/10 w-full ring-1"
          disabled={action.busy}
          onClick={() => answer(false)}
        >
          Don't trust
        </Button>
      </div>
      <p className="text-muted-foreground text-xs">
        If you don't trust it, every session here asks before it does anything. You can change this
        in the project's settings.
      </p>
    </div>
  );
}
