import {
  ArrowLeft,
  Check,
  Folder,
  FolderOpen,
  Link,
  Plus,
  Search,
  X,
} from "@openai/apps-sdk-ui/components/Icon";
import { useEffect, useEffectEvent, useId, useMemo, useState, type FormEvent, type KeyboardEvent, type ReactNode } from "react";

import { ErrorLine, errorText } from "@/app/dialogs/fields";
import { DialogPill } from "@/app/dialogs/pills";
import { Spinner } from "@/components/glyphs/spinner";
import { Dialog, DialogClose, DialogContent, DialogDescription, DialogTitle } from "@/components/ui/dialog";
import { useNow } from "@/hooks/use-now";
import { pickFolder } from "@/ipc/client";
import type { FolderCheck, RepoSuggestion } from "@/ipc/generated";
import { formatLastUsed, shortPath } from "@/lib/format";
import { cn } from "@/lib/utils";
import {
  addProject,
  checkFolder,
  closeAddProject,
  cloneProject,
  findRepos,
  showProject,
  useAddProject,
} from "@/state/addProject";
import { toast } from "@/state/toasts";

/*
 * Add project, drawn to the user's design: pick a git repository found on this computer (or
 * choose a folder), or clone one from a URL, or start a new one. Its colours and type are the
 * redesigned Settings pages' (`settings-*`).
 */

/** Where clones and new repositories go unless the user changes it (made when missing). */
const DEFAULT_PARENT = "~/code/";

export function AddProjectDialog() {
  const open = useAddProject((s) => s.open);
  const path = useAddProject((s) => s.path);
  const opening = useAddProject((s) => s.opening);
  return (
    <Dialog open={open} onOpenChange={(next) => !next && closeAddProject()}>
      <DialogContent
        data-testid="add-project-dialog"
        showCloseButton={false}
        className="bg-settings-surface border-settings-border rounded-design-dialog max-w-add-project shadow-design-dialog text-settings-text gap-0 border p-0 ring-0"
      >
        {/* Remounted per opening so it starts over. */}
        {open && <AddProject key={opening} initialPath={path} />}
      </DialogContent>
    </Dialog>
  );
}

type View = { kind: "pick" } | { kind: "clone" } | { kind: "new"; parent: string; name: string };

function AddProject({ initialPath }: { initialPath: string }) {
  const [view, setView] = useState<View>({ kind: "pick" });
  const back = () => setView({ kind: "pick" });
  switch (view.kind) {
    case "pick":
      return (
        <PickView
          initialPath={initialPath}
          onClone={() => setView({ kind: "clone" })}
          onNew={(parent = DEFAULT_PARENT, name = "") => setView({ kind: "new", parent, name })}
        />
      );
    case "clone":
      return <CloneView onBack={back} />;
    case "new":
      return <NewRepoView initialParent={view.parent} initialName={view.name} onBack={back} />;
  }
}

// ----- the parts -----------------------------------------------------------------------------

/** The title, its line and the close button; with `onBack`, a back arrow before them. */
function Header({ title, line, onBack }: { title: string; line: string; onBack?: (() => void) | undefined }) {
  return (
    <div className={cn("flex gap-3.5 pt-6 pr-5 pb-5", onBack ? "pl-6" : "pl-7")}>
      {onBack && (
        <button
          type="button"
          aria-label="Back"
          onClick={onBack}
          className="bg-settings-raised text-settings-text hover:bg-settings-border rounded-capsule focus-visible:ring-settings-accent flex size-7 shrink-0 items-center justify-center outline-none focus-visible:ring-2"
        >
          <ArrowLeft className="size-icon-sm" />
        </button>
      )}
      <div className="flex min-w-0 flex-1 flex-col gap-1">
        <DialogTitle className="text-dialog-title text-settings-text font-medium">{title}</DialogTitle>
        <DialogDescription className="text-settings-text-secondary text-settings-small">{line}</DialogDescription>
      </div>
      <DialogClose
        aria-label="Close"
        className="text-settings-text-secondary hover:bg-settings-raised hover:text-settings-text rounded-capsule focus-visible:ring-settings-accent flex size-7 shrink-0 items-center justify-center outline-none focus-visible:ring-2"
      >
        <X className="size-design-icon-close" />
      </DialogClose>
    </div>
  );
}

/** The bar along the bottom: `start` on the left, Cancel and the main button on the right. */
function Footer({ start, children }: { start?: ReactNode; children: ReactNode }) {
  return (
    <div className="border-settings-divider flex items-center gap-2 border-t py-4 pr-6 pl-5">
      <div className="flex min-w-0 flex-1 items-center gap-1">{start}</div>
      {children}
    </div>
  );
}

function CancelButton({ label = "Cancel" }: { label?: string }) {
  return (
    <DialogPill look="quiet" onClick={closeAddProject}>
      {label}
    </DialogPill>
  );
}

/** A field's label, with an optional note on the right. */
function FieldLabel({ htmlFor, label, aside }: { htmlFor?: string; label: string; aside?: string | undefined }) {
  return (
    <div className="flex items-center justify-between gap-4">
      <label htmlFor={htmlFor} className="text-settings-text text-settings-small font-medium">
        {label}
      </label>
      {aside && <span className="text-settings-text-tertiary text-settings-meta">{aside}</span>}
    </div>
  );
}

/** A 42-high box for a field: its icon, then its content. */
const BOX =
  "bg-settings-inset rounded-design-row flex h-10.5 items-center gap-2.5 border [&>svg]:size-icon-sm [&>svg]:shrink-0 [&>svg]:text-settings-text-tertiary";

/**
 * Save to: the folder it goes in (`parent`, dimmer) and its own name a step brighter, with
 * Change… to choose another parent folder.
 */
function SaveTo({
  parent,
  name,
  aside,
  disabled,
  onParent,
  onError,
}: {
  parent: string;
  name: string;
  aside?: string;
  disabled?: boolean;
  onParent: (parent: string) => void;
  onError: (error: string) => void;
}) {
  const change = () => {
    pickFolder(parent)
      .then((picked) => {
        if (picked) onParent(`${shortPath(picked).replace(/[/\\]+$/, "")}/`);
      })
      .catch((cause: unknown) => onError(errorText(cause)));
  };
  return (
    <div className="flex flex-col gap-2">
      <FieldLabel label="Save to" aside={aside} />
      <div className={cn(BOX, "border-settings-divider pr-1.5 pl-3")}>
        <Folder />
        <p data-slot="save-to" className="text-settings-meta flex min-w-0 flex-1 font-mono">
          <span className="text-settings-text-tertiary min-w-0 truncate">{parent}</span>
          <span className="text-settings-text shrink-0">{name}</span>
        </p>
        <button
          type="button"
          disabled={disabled}
          onClick={change}
          className="bg-settings-raised border-settings-ring text-settings-text hover:bg-settings-border rounded-capsule text-settings-small focus-visible:ring-settings-accent flex h-7 shrink-0 items-center border px-3 font-medium outline-none focus-visible:ring-2 disabled:opacity-50"
        >
          Change…
        </button>
      </div>
    </div>
  );
}

/** The folder a path ends in, and the folder it is in ("~/code/" and "app" for ~/code/app). */
function splitPath(path: string): { parent: string; name: string } {
  const trimmed = path.replace(/[/\\]+$/, "");
  const cut = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\")) + 1;
  return { parent: trimmed.slice(0, cut), name: trimmed.slice(cut) };
}

// ----- Pick ----------------------------------------------------------------------------------

/** A row of the list: a repository found, or a folder the user chose. */
type Row = {
  path: string;
  name: string;
  lastUsedMs: number | null;
  projectId: string | null;
  /** A folder in no repository: adding it starts one there. */
  plain?: boolean;
};

function rowOf(repo: RepoSuggestion): Row {
  return { path: repo.path, name: repo.name, lastUsedMs: repo.lastUsedMs, projectId: repo.projectId };
}

function PickView({
  initialPath,
  onClone,
  onNew,
}: {
  initialPath: string;
  onClone: () => void;
  onNew: (parent?: string, name?: string) => void;
}) {
  const [repos, setRepos] = useState<Row[] | null>(null);
  // A folder chosen (or dropped, or opened), first in the list.
  const [chosen, setChosen] = useState<Row | null>(null);
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const now = useNow(60_000);

  useEffect(() => {
    let live = true;
    findRepos()
      .then((found) => {
        if (live) setRepos(found.map(rowOf));
      })
      .catch((cause: unknown) => {
        if (live) {
          setRepos([]);
          setError(errorText(cause));
        }
      });
    return () => {
      live = false;
    };
  }, []);

  /** Puts a checked folder at the top of the list and picks it; a project shows itself. */
  const apply = (check: FolderCheck) => {
    setError(null);
    switch (check.kind) {
      case "repo":
        if (check.projectId) {
          closeAddProject();
          showProject(check.projectId);
          return;
        }
        setChosen({ path: check.root, name: check.name, lastUsedMs: null, projectId: null });
        setSelected(check.root);
        return;
      case "plain":
        setChosen({ path: check.path, name: check.name, lastUsedMs: null, projectId: null, plain: true });
        setSelected(check.path);
        return;
      case "missing": {
        const { parent, name } = splitPath(shortPath(check.path));
        onNew(parent, name);
        return;
      }
      case "invalid":
        setError(check.reason);
    }
  };
  const take = (path: string) =>
    checkFolder(path).then(apply, (cause: unknown) => setError(errorText(cause)));

  // The folder the dialog opened on (dropped, or opened from the file manager).
  const applyInitial = useEffectEvent(apply);
  useEffect(() => {
    if (!initialPath) return;
    let live = true;
    checkFolder(initialPath).then(
      (check) => live && applyInitial(check),
      (cause: unknown) => live && setError(errorText(cause)),
    );
    return () => {
      live = false;
    };
  }, [initialPath]);

  const rows = useMemo(() => {
    const all = [...(chosen ? [chosen] : []), ...(repos ?? []).filter((repo) => repo.path !== chosen?.path)];
    const words = query.trim().toLowerCase();
    if (!words) return all;
    return all.filter((row) => row.name.toLowerCase().includes(words) || shortPath(row.path).toLowerCase().includes(words));
  }, [chosen, repos, query]);
  const pickable = rows.filter((row) => !row.projectId);
  // The row picked, else the first that can be.
  const current = pickable.find((row) => row.path === selected) ?? pickable[0] ?? null;

  const add = async (row: Row | null = current) => {
    if (!row || busy) return;
    setBusy(true);
    setError(null);
    try {
      const project = await addProject(row.path, "", row.plain ?? false);
      closeAddProject();
      showProject(project.id);
    } catch (cause) {
      setError(errorText(cause));
      setBusy(false);
    }
  };

  const onSearchKey = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "Enter") {
      event.preventDefault();
      void add();
    } else if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      if (pickable.length === 0) return;
      const at = current ? pickable.indexOf(current) : -1;
      const next = event.key === "ArrowDown" ? Math.min(at + 1, pickable.length - 1) : Math.max(at - 1, 0);
      setSelected(pickable[next]!.path);
    }
  };

  const choose = () => {
    setError(null);
    pickFolder()
      .then((picked) => (picked ? take(picked) : undefined))
      .catch((cause: unknown) => setError(errorText(cause)));
  };

  return (
    <div className="flex flex-col">
      <Header title="Add project" line="Pick a git repository for your sessions to work in." />
      <div className="flex items-center gap-2.5 px-7">
        <label className="bg-settings-inset border-settings-divider rounded-capsule focus-within:border-settings-ring flex h-9 min-w-0 flex-1 items-center gap-2 border px-3.5">
          <Search className="size-icon-sm text-settings-text-tertiary shrink-0" />
          <input
            value={query}
            autoFocus
            spellCheck={false}
            autoComplete="off"
            aria-label="Search repositories"
            placeholder="Search repositories"
            onChange={(event) => setQuery(event.target.value)}
            onKeyDown={onSearchKey}
            className="text-settings-text text-settings-small placeholder:text-settings-text-tertiary min-w-0 flex-1 bg-transparent outline-none"
          />
        </label>
        <DialogPill look="secondary" className="h-9" onClick={choose}>
          <FolderOpen className="size-icon-sm" />
          Choose folder…
        </DialogPill>
      </div>
      <div className="flex flex-col gap-2 px-4 pt-5.5 pb-4">
        <div className="flex items-center justify-between px-3">
          <span className="text-settings-text-tertiary text-settings-small">On this Mac</span>
          <span className="text-settings-text-tertiary text-settings-meta">Most recent first</span>
        </div>
        <div
          role="radiogroup"
          aria-label="On this Mac"
          className="max-h-add-project-list flex flex-col gap-0.5 overflow-y-auto"
        >
          {rows.map((row) => (
            <RepoRow
              key={row.path}
              row={row}
              now={now}
              chosen={row === current}
              onPick={() => setSelected(row.path)}
              onAdd={() => void add(row)}
            />
          ))}
          {rows.length === 0 && (
            <p className="text-settings-text-tertiary text-settings-small flex h-14.5 items-center px-3">
              {repos === null
                ? "Looking for repositories…"
                : query.trim()
                  ? "No repository matches."
                  : "No git repositories found. Choose a folder, or clone or start one."}
            </p>
          )}
        </div>
        {error && (
          <div className="px-3">
            <ErrorLine error={error} />
          </div>
        )}
      </div>
      <Footer
        start={
          <>
            <DialogPill look="quiet" onClick={onClone}>
              <Link className="size-design-icon-sm" />
              Clone from URL
            </DialogPill>
            <DialogPill look="quiet" onClick={() => onNew()}>
              <Plus className="size-design-icon-sm" />
              New repository
            </DialogPill>
          </>
        }
      >
        <CancelButton />
        <DialogPill look="primary" disabled={!current || busy} onClick={() => void add()}>
          {busy && <Spinner className="size-icon-xs animate-spin" />}
          {current ? `Add ${current.name}` : "Add project"}
        </DialogPill>
      </Footer>
    </div>
  );
}

/** A repository in the list: its mark, name, path and last use, and its radio or Added. */
function RepoRow({
  row,
  now,
  chosen,
  onPick,
  onAdd,
}: {
  row: Row;
  now: number;
  chosen: boolean;
  onPick: () => void;
  onAdd: () => void;
}) {
  const added = row.projectId !== null;
  return (
    <button
      type="button"
      role="radio"
      aria-checked={chosen}
      disabled={added}
      tabIndex={chosen ? 0 : -1}
      onClick={onPick}
      onDoubleClick={onAdd}
      className={cn(
        "rounded-design-row focus-visible:ring-settings-accent flex h-14.5 w-full shrink-0 cursor-default items-center gap-3 border px-3 text-left outline-none focus-visible:ring-2",
        chosen ? "bg-design-row-chosen border-settings-ring" : "hover:bg-design-row-chosen border-transparent",
        added && "opacity-55",
      )}
    >
      <span className="bg-settings-chosen rounded-design-mark flex size-8.5 shrink-0 items-center justify-center">
        <Folder className={cn("size-icon-md", chosen ? "text-settings-text" : "text-settings-text-secondary")} />
      </span>
      <span className="flex min-w-0 flex-1 flex-col gap-0.75">
        <span className="text-settings-text text-settings-body truncate font-medium">{row.name}</span>
        <span className="text-settings-text-tertiary text-settings-meta truncate font-mono">{shortPath(row.path)}</span>
      </span>
      {added ? (
        <span className="bg-settings-border rounded-design-badge flex shrink-0 items-center gap-1.25 px-2 py-0.75">
          <Check className="size-design-icon-xs text-settings-text-secondary" />
          <span className="text-design-badge-text text-settings-meta font-medium">Added</span>
        </span>
      ) : (
        <>
          <span className="text-settings-text-tertiary text-settings-meta shrink-0">
            {row.plain ? "Not a repository" : row.lastUsedMs ? formatLastUsed(row.lastUsedMs, now) : ""}
          </span>
          <span
            aria-hidden
            className={cn(
              "rounded-capsule flex size-4.5 shrink-0 items-center justify-center",
              chosen ? "bg-settings-accent text-design-on-accent" : "shadow-design-radio",
            )}
          >
            {chosen && <Check className="size-icon-xs" />}
          </span>
        </>
      )}
    </button>
  );
}

// ----- Clone from URL ------------------------------------------------------------------------

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

function CloneView({ onBack }: { onBack: () => void }) {
  const [url, setUrl] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const folder = folderFromUrl(url);
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (busy || !folder) return;
    setBusy(true);
    try {
      const project = await cloneProject(cloneUrl(url), DEFAULT_PARENT, folder, "");
      closeAddProject();
      showProject(project.id);
      toast(`Cloned ${project.name}`);
    } catch (cause) {
      setError(errorText(cause));
      setBusy(false);
    }
  };
  return (
    <form onSubmit={(event) => void submit(event)} className="flex flex-col">
      <Header title="Clone from URL" line="Paste a link to a git repository." onBack={onBack} />
      <div className="flex flex-col gap-2 px-7 pb-7">
        <input
          value={url}
          autoFocus
          aria-label="Repository URL"
          onChange={(event) => setUrl(event.target.value)}
          className="bg-settings-inset border-settings-divider rounded-design-row text-settings-meta h-10.5 border px-3 font-mono outline-none"
        />
        <ErrorLine error={error} />
      </div>
      <Footer>
        <CancelButton />
        <DialogPill look="primary" type="submit" disabled={busy}>
          Clone and add
        </DialogPill>
      </Footer>
    </form>
  );
}

// ----- New repository ------------------------------------------------------------------------

function NewRepoView({
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
  const [busy, setBusy] = useState(false);
  const folder = name.trim();

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (busy) return;
    if (!folder || folder === "." || folder === ".." || /[/\\]/.test(folder)) {
      setError(folder ? `"${folder}" can't be a folder name.` : "Name the repository.");
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
          <div className={cn(BOX, "border-settings-divider focus-within:border-settings-accent px-3")}>
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
              onChange={(event) => {
                setName(event.target.value);
                setError(null);
              }}
              className="text-settings-text text-settings-meta placeholder:text-settings-text-tertiary caret-settings-link min-w-0 flex-1 bg-transparent font-mono outline-none"
            />
          </div>
        </div>
        <SaveTo parent={parent} name={folder} disabled={busy} onParent={setParent} onError={setError} />
        <ErrorLine error={error} />
      </div>
      <Footer>
        <CancelButton />
        <DialogPill look="primary" type="submit" disabled={busy}>
          {busy && <Spinner className="size-icon-xs animate-spin" />}
          Create and add
        </DialogPill>
      </Footer>
    </form>
  );
}
