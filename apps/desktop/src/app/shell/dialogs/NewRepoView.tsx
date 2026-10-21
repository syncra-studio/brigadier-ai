import { useId, useState, type FormEvent } from "react";

import { BOX, BOX_INPUT, CancelButton, FieldLabel, Footer, Header, SaveTo } from "@/app/shell/dialogs/addProjectParts";
import { ErrorLine, errorText } from "@/app/shell/dialogs/fields";
import { Plus, Spinner } from "@/components/icons";
import { Button } from "@/components/controls";
import { cn } from "@/lib/utils";
import { addProject, closeAddProject, showProject } from "@/state/addProject";

/** Add project › New repository: its name and where it goes; Brigadier starts git in it. */
export function NewRepoView({
  initialParent,
  initialName,
  onBack,
}: {
  initialParent: string;
  initialName: string;
  onBack: () => void;
}) {
  const id = useId();
  const [name, setName] = useState(initialName);
  const [parent, setParent] = useState(initialParent);
  const [error, setError] = useState<string | null>(null);
  // The error is about the name itself: its field shows it.
  const [invalid, setInvalid] = useState(false);
  const [busy, setBusy] = useState(false);
  const folder = name.trim();

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (busy) return;
    if (!folder || folder === "." || folder === ".." || /[/\\]/.test(folder)) {
      setError(folder ? `"${folder}" can't be a folder name.` : "Name the repository.");
      setInvalid(true);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const project = await addProject(`${parent}${folder}`, "", true);
      closeAddProject();
      showProject(project.id);
    } catch (cause) {
      setError(errorText(cause));
      setBusy(false);
    }
  };

  return (
    <form onSubmit={(event) => void submit(event)} className="flex flex-col">
      <Header
        title="New repository"
        line="Name it and choose where it goes. Brigadier starts git in it and adds it."
        onBack={onBack}
      />
      <div className="flex flex-col gap-5.5 px-7 pt-1 pb-7">
        <div className="flex flex-col gap-2">
          <FieldLabel htmlFor={`${id}-name`} label="Name" />
          <div className={cn(BOX, "px-3")}>
            <Plus />
            <input
              id={`${id}-name`}
              value={name}
              autoFocus
              spellCheck={false}
              autoComplete="off"
              maxLength={200}
              disabled={busy}
              placeholder="my-project"
              aria-invalid={invalid || undefined}
              onChange={(event) => {
                setName(event.target.value);
                setError(null);
                setInvalid(false);
              }}
              className={BOX_INPUT}
            />
          </div>
        </div>
        <SaveTo parent={parent} name={folder} disabled={busy} onParent={setParent} onError={setError} />
        <ErrorLine error={error} className="leading-4.25" />
      </div>
      <Footer>
        <CancelButton />
        <Button size="sm" type="submit" disabled={busy}>
          {busy && <Spinner className="size-icon-xs animate-spin" />}
          Create and add
        </Button>
      </Footer>
    </form>
  );
}
