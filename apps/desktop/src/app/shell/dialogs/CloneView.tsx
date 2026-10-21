import { useEffect, useId, useRef, useState, type FormEvent } from "react";

import { BOX, BOX_INPUT, CancelButton, DEFAULT_PARENT, FieldLabel, Footer, Header, SaveTo } from "@/app/shell/dialogs/addProjectParts";
import { ErrorLine, errorText } from "@/app/shell/dialogs/fields";
import { Link, Spinner } from "@/components/icons";
import { Button } from "@/components/controls";
import { cn } from "@/lib/utils";
import { closeAddProject, cloneProject, showProject } from "@/state/addProject";
import { toast } from "@/state/toasts";

/** `owner/name` is a GitHub repository; anything else is taken as typed. */
function cloneUrl(typed: string): string {
  const url = typed.trim();
  return /^[\w.-]+\/[\w.-]+$/.test(url) ? `https://github.com/${url.replace(/\.git$/, "")}.git` : url;
}

/** The folder a clone of `url` goes in: its last path part, without `.git`. */
function folderFromUrl(url: string): string {
  const last = url.trim().replace(/[/\\]+$/, "").split(/[/:\\]/).pop() ?? "";
  return last.replace(/\.git$/, "");
}

/**
 * Add project › Clone from URL: the repository's URL (`owner/name` for GitHub) and where the
 * clone goes; the clone goes on if the dialog closes, its outcome then shown as a toast.
 */
export function CloneView({ onBack }: { onBack: () => void }) {
  const id = useId();
  const [url, setUrl] = useState("");
  const [parent, setParent] = useState(DEFAULT_PARENT);
  const [error, setError] = useState<string | null>(null);
  // The error is about the URL itself: its field shows it.
  const [invalid, setInvalid] = useState(false);
  const [busy, setBusy] = useState(false);
  const mounted = useRef(false);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  const folder = folderFromUrl(url);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (busy) return;
    if (!url.trim()) {
      setError("Paste the repository's URL.");
      setInvalid(true);
      return;
    }
    if (!folder) {
      setError("That URL doesn't name a repository.");
      setInvalid(true);
      return;
    }
    setBusy(true);
    setError(null);
    // The clone goes on if the dialog closes; its outcome then shows as a toast.
    try {
      const project = await cloneProject(cloneUrl(url), parent, folder, "");
      if (mounted.current) closeAddProject();
      showProject(project.id);
      toast(`Cloned ${project.name}`);
    } catch (cause) {
      if (mounted.current) {
        setError(errorText(cause));
        setBusy(false);
      } else {
        toast(`Could not clone ${folder}: ${errorText(cause)}`, { tone: "error" });
      }
    }
  };

  return (
    <form onSubmit={(event) => void submit(event)} className="flex flex-col">
      <Header
        title="Clone from URL"
        line="Paste a link to a git repository. We'll copy it to your Mac and add it."
        onBack={busy ? undefined : onBack}
      />
      <div className="flex flex-col gap-5.5 px-7 pt-1 pb-7">
        <div className="flex flex-col gap-2">
          <FieldLabel htmlFor={`${id}-url`} label="Repository URL" />
          <div className={cn(BOX, "px-3")}>
            <Link />
            <input
              id={`${id}-url`}
              value={url}
              autoFocus
              spellCheck={false}
              autoComplete="off"
              disabled={busy}
              placeholder="https://github.com/owner/repo.git"
              aria-invalid={invalid || undefined}
              onChange={(event) => {
                setUrl(event.target.value);
                setError(null);
                setInvalid(false);
              }}
              className={BOX_INPUT}
            />
          </div>
        </div>
        <SaveTo
          parent={parent}
          name={folder}
          aside="Named after the repository"
          disabled={busy}
          onParent={setParent}
          onError={setError}
        />
        <ErrorLine error={error} className="leading-4.25" />
      </div>
      <Footer>
        <CancelButton label={busy ? "Close" : "Cancel"} />
        <Button size="sm" type="submit" disabled={busy}>
          {busy && <Spinner className="size-icon-xs animate-spin" />}
          {busy ? "Cloning…" : "Clone and add"}
        </Button>
      </Footer>
    </form>
  );
}
