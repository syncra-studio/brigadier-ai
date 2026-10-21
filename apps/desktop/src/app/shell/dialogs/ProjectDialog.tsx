import { useId, useState, type FormEvent } from "react";

import { useAction } from "@/app/conversation/useAction";
import { Field } from "@/app/shell/dialogs/Field";
import { errorText, PROJECT_DIALOG_CARD, PROJECT_DIALOG_TITLE } from "@/app/shell/dialogs/fields";
import { Button, Input, Switch, ErrorLine } from "@/components/controls";
import { Dialog, DialogClose, DialogContent, DialogTitle } from "@/components/overlays";
import { Plus, Trash, X } from "@/components/icons";
import { pickFolder } from "@/ipc/client";
import type { Project } from "@/ipc/generated";
import { shortPath } from "@/lib/format";
import { cn } from "@/lib/utils";
import { updateProject } from "@/state/actions";
import { useApp } from "@/state/store";
import { folderTrust, setFolderTrust } from "@/state/trust";

export type ProjectDialogProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  project: Project | null;
};

/** Edits a project's name, repository, folder trust and secrets (new projects: `AddProjectDialog`). */
export function ProjectDialog(props: ProjectDialogProps) {
  return (
    <Dialog open={props.open && props.project !== null} onOpenChange={props.onOpenChange}>
      <DialogContent showCloseButton={false} className={cn(PROJECT_DIALOG_CARD, "max-w-project-dialog gap-7 p-6")}>
        {/* Remounted per opening so the fields start from the project. */}
        {props.open && props.project && (
          <SettingsForm project={props.project} onOpenChange={props.onOpenChange} />
        )}
      </DialogContent>
    </Dialog>
  );
}

/**
 * Whether the project's (saved) folder is trusted, applied at once. A folder never asked about
 * shows off until answered.
 */
function TrustRow({ projectId, folder }: { projectId: string; folder: string }) {
  const id = useId();
  const trusted = useApp((s) => folderTrust(s.projects[projectId], folder) === true);
  const save = useAction();
  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-center gap-4">
        <div className="flex min-w-0 flex-1 flex-col gap-0.75">
          <label htmlFor={`${id}-trusted`} className="text-foreground text-label leading-4.25 font-medium">
            Trust this folder
          </label>
          <p className="text-muted-foreground text-xs leading-4.25">Agents run commands here without asking first.</p>
        </div>
        <Switch
          id={`${id}-trusted`}
          checked={trusted}
          disabled={save.busy}
          onCheckedChange={(on) => save.run(() => setFolderTrust(projectId, folder, on))}
        />
      </div>
      <ErrorLine error={save.error} className="leading-4.25" />
    </div>
  );
}

/**
 * The repository: its path in a field, with Change… to pick another folder (Choose… when the
 * project has none).
 */
function RepositoryField({ id, value, onChange }: { id: string; value: string; onChange: (path: string) => void }) {
  const [error, setError] = useState<string | null>(null);
  const choose = () => {
    setError(null);
    pickFolder(value || undefined)
      .then((picked) => {
        if (picked) onChange(picked);
      })
      .catch((cause: unknown) => setError(errorText(cause)));
  };
  return (
    <>
      <div className="bg-muted/60 rounded-control flex h-9 items-center gap-2 pr-1 pl-3">
        <span
          id={id}
          className={value ? "text-foreground min-w-0 flex-1 truncate font-mono text-xs" : "text-muted-foreground text-label min-w-0 flex-1 truncate leading-4"}
        >
          {value ? shortPath(value) : "No repository chosen"}
        </span>
        <Button type="button" variant="ghost" size="sm" aria-describedby={id} onClick={choose}>
          {value ? "Change" : "Choose…"}
        </Button>
      </div>
      <ErrorLine error={error} className="leading-4.25" />
    </>
  );
}

/** Why a secret file path is not acceptable, or `null`. */
function secretFileProblem(path: string): string | null {
  if (!path) return "A secret file path can't be empty.";
  if (path.startsWith("/") || path.startsWith("~") || /^[A-Za-z]:[\\/]/.test(path)) {
    return `"${path}" must be relative to the repository root.`;
  }
  if (path.split(/[\\/]/).includes("..")) return `"${path}" must stay inside the repository.`;
  return null;
}

function SettingsForm({
  project,
  onOpenChange,
}: {
  project: Project;
  onOpenChange: (open: boolean) => void;
}) {
  const id = useId();
  const initialRepo = project.repos[0]?.path ?? "";
  const [name, setName] = useState(project.name);
  const [repo, setRepo] = useState(initialRepo);
  const [secrets, setSecrets] = useState<string[]>(project.prefs.secretFiles);
  // The secret file row the error is about, outlined in red.
  const [badRow, setBadRow] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    const trimmedName = name.trim();
    if (!trimmedName) {
      setError("Name can't be empty.");
      return;
    }
    const files = secrets.map((file) => file.trim());
    const problems = files.map(secretFileProblem);
    const bad = problems.findIndex((entry) => entry !== null);
    if (bad >= 0) {
      setBadRow(bad);
      setError(problems[bad]!);
      return;
    }
    const secretsChanged =
      files.length !== project.prefs.secretFiles.length ||
      files.some((file, index) => file !== project.prefs.secretFiles[index]);
    setBusy(true);
    setBadRow(null);
    setError(null);
    try {
      await updateProject(project.id, {
        name: trimmedName === project.name ? null : trimmedName,
        repos: repo === initialRepo ? null : [repo],
        prefs: secretsChanged ? { ...project.prefs, secretFiles: files } : null,
      });
      onOpenChange(false);
    } catch (cause) {
      setError(errorText(cause));
    } finally {
      setBusy(false);
    }
  };

  return (
    <form onSubmit={(event) => void submit(event)} className="flex flex-col gap-7">
      <div className="flex flex-col gap-5.5">
        <div className="flex items-center gap-3">
          <DialogTitle className={cn(PROJECT_DIALOG_TITLE, "min-w-0 flex-1")}>Project settings</DialogTitle>
          <DialogClose asChild>
            <Button type="button" variant="ghost" size="icon-sm" className="text-muted-foreground" aria-label="Close">
              <X />
            </Button>
          </DialogClose>
        </div>
        <Field label="Name" htmlFor={`${id}-name`}>
          <Input
            id={`${id}-name`}
            value={name}
            autoFocus
            maxLength={200}
            className="h-9 px-3"
            onChange={(event) => setName(event.target.value)}
          />
        </Field>
        <Field label="Repository">
          <RepositoryField id={`${id}-repo`} value={repo} onChange={setRepo} />
        </Field>
        {initialRepo && <TrustRow projectId={project.id} folder={initialRepo} />}
        <Field label="Secret files" aside="Copied into every worktree, never shown">
          <ul className="flex flex-col gap-1.5" aria-label="Secret files">
            {secrets.map((file, index) => (
              <li key={index} className="flex items-center gap-1">
                <Input
                  value={file}
                  spellCheck={false}
                  autoComplete="off"
                  placeholder=".env.local"
                  aria-label={`Secret file ${index + 1}`}
                  aria-invalid={badRow === index || undefined}
                  className="aria-invalid:border-destructive/60 h-9 px-3 font-mono text-xs aria-invalid:ring-0"
                  onChange={(event) => {
                    if (badRow === index) setBadRow(null);
                    setSecrets((current) =>
                      current.map((entry, at) => (at === index ? event.target.value : entry)),
                    );
                  }}
                />
                <Button
                  type="button"
                  variant="ghost"
                  size="icon-md"
                  className="text-muted-foreground"
                  aria-label={`Remove ${file || "this file"}`}
                  onClick={() => {
                    setBadRow(null);
                    setSecrets((current) => current.filter((_, at) => at !== index));
                  }}
                >
                  <Trash />
                </Button>
              </li>
            ))}
          </ul>
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="self-start"
            onClick={() => setSecrets((current) => [...current, ""])}
          >
            <Plus />
            Add file
          </Button>
        </Field>
        <ErrorLine error={error} className="leading-4.25" />
      </div>
      <div className="flex justify-end gap-2">
        <Button type="button" variant="ghost" onClick={() => onOpenChange(false)}>
          Cancel
        </Button>
        <Button type="submit" disabled={busy}>
          {busy ? "Saving…" : "Save"}
        </Button>
      </div>
    </form>
  );
}
