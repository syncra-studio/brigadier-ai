import {
  useEffect,
  useEffectEvent,
  useId,
  useMemo,
  useRef,
  useState,
  type FormEvent,
  type KeyboardEvent,
  type ReactNode,
  type RefObject,
} from "react";

import { ErrorLine, errorText } from "@/app/dialogs/fields";
import { SettingsSearch } from "@/app/settings/parts";
import { Spinner } from "@/components/glyphs/spinner";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogClose, DialogContent, DialogDescription, DialogTitle } from "@/components/ui/dialog";
import {
  ArrowLeft,
  Check,
  Folder,
  FolderOpen,
  Link,
  Plus,
  X,
} from "@/components/icons";
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
 * Add project, laid out to the user's design and drawn with the app's dialog parts: pick a git
 * repository found on this computer (or choose a folder), or clone one from a URL, or start a
 * new one.
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
        className="max-w-add-project top-add-project-top origin-top translate-y-0 gap-0 p-0 motion-reduce:animate-none"
      >
        {/* Its top edge stays put (#384): a view or the list loading changes only its height,
            which moves the bottom edge. Remounted per opening so it starts over. */}
        {open && <AddProject key={opening} initialPath={path} />}
      </DialogContent>
    </Dialog>
  );
}

type View = { kind: "pick" } | { kind: "clone" } | { kind: "new"; parent: string; name: string };

function AddProject({ initialPath }: { initialPath: string }) {
  const [view, setView] = useState<View>({ kind: "pick" });
  // The repositories found, kept for the list's return from Clone or New repository.
  const found = useRef<Row[] | null>(null);
  const back = () => setView({ kind: "pick" });
  switch (view.kind) {
    case "pick":
      return (
        <PickView
          initialPath={initialPath}
          found={found}
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
        <Button variant="ghost" size="icon-sm" aria-label="Back" onClick={onBack}>
          <ArrowLeft />
        </Button>
      )}
      <div className="flex min-w-0 flex-1 flex-col gap-1">
        <DialogTitle>{title}</DialogTitle>
        <DialogDescription>{line}</DialogDescription>
      </div>
      <DialogClose asChild>
        <Button variant="ghost" size="icon-sm" aria-label="Close">
          <X />
        </Button>
      </DialogClose>
    </div>
  );
}

/** The bar along the bottom: `start` on the left, Cancel and the main button on the right. */
function Footer({ start, children }: { start?: ReactNode; children: ReactNode }) {
  return (
    <div className="border-divider flex items-center gap-2 border-t py-4 pr-6 pl-5">
      <div className="flex min-w-0 flex-1 items-center gap-1">{start}</div>
      {children}
    </div>
  );
}

function CancelButton({ label = "Cancel" }: { label?: string }) {
  return (
    <Button variant="ghost" size="sm" onClick={closeAddProject}>
      {label}
    </Button>
  );
}

/** A field's label, with an optional note on the right. */
function FieldLabel({ htmlFor, label, aside }: { htmlFor?: string; label: string; aside?: string | undefined }) {
  return (
    <div className="flex items-center justify-between gap-4">
      <label htmlFor={htmlFor} className="text-foreground text-xs font-medium">
        {label}
      </label>
      {aside && <span className="text-muted-foreground text-xs">{aside}</span>}
    </div>
  );
}

/** A 42-high box for a field, in the app's field look (and the Input's when invalid): its icon, then its content. */
const BOX =
  "bg-muted/60 focus-within:bg-background rounded-control flex h-10.5 items-center gap-2.5 border border-transparent transition-colors [&>svg]:size-icon-sm [&>svg]:shrink-0 [&>svg]:text-muted-foreground has-aria-invalid:border-destructive/50 has-aria-invalid:ring-destructive/40 has-aria-invalid:ring-1";

/** A field's own input inside its box. */
const BOX_INPUT = "text-foreground placeholder:text-muted-foreground min-w-0 flex-1 bg-transparent font-mono text-xs outline-none";

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
      <div className={cn(BOX, "pr-1.5 pl-3")}>
        <Folder />
        <p data-slot="save-to" className="flex min-w-0 flex-1 font-mono text-xs">
          <span className="text-muted-foreground min-w-0 truncate">{parent}</span>
          <span className="text-foreground shrink-0">{name}</span>
        </p>
        <Button variant="outline" size="sm" disabled={disabled} onClick={change}>
          Change…
        </Button>
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
  found,
  onClone,
  onNew,
}: {
  initialPath: string;
  found: RefObject<Row[] | null>;
  onClone: () => void;
  onNew: (parent?: string, name?: string) => void;
}) {
  const [repos, setRepos] = useState<Row[] | null>(found.current);
  // A folder chosen (or dropped, or opened), first in the list.
  const [chosen, setChosen] = useState<Row | null>(null);
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const now = useNow(60_000);

  useEffect(() => {
    if (found.current) return;
    let live = true;
    findRepos()
      .then((list) => {
        found.current = list.map(rowOf);
        if (live) setRepos(found.current);
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
  }, [found]);

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
        <SettingsSearch
          label="Search repositories"
          value={query}
          autoFocus
          className="h-9"
          onChange={setQuery}
          onKeyDown={onSearchKey}
        />
        <Button variant="outline" className="h-9" onClick={choose}>
          <FolderOpen />
          Choose folder…
        </Button>
      </div>
      <div className="flex flex-col gap-2 px-4 pt-5.5 pb-4">
        <div className="flex items-center justify-between px-3">
          <span className="text-muted-foreground text-xs">On this Mac</span>
          <span className="text-muted-foreground text-xs">Most recent first</span>
        </div>
        {/* While the scan runs the list holds its five rows' room. */}
        <div
          role="radiogroup"
          aria-label="On this Mac"
          className={cn("max-h-add-project-list flex flex-col gap-0.5 overflow-y-auto", repos === null && "min-h-add-project-list")}
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
            <p className="text-muted-foreground flex h-14.5 items-center px-3 text-sm">
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
            <Button variant="ghost" size="sm" onClick={onClone}>
              <Link />
              Clone from URL
            </Button>
            <Button variant="ghost" size="sm" onClick={() => onNew()}>
              <Plus />
              New repository
            </Button>
          </>
        }
      >
        <CancelButton />
        <Button size="sm" disabled={!current || busy} onClick={() => void add()}>
          {busy && <Spinner className="size-icon-xs animate-spin" />}
          {current ? `Add ${current.name}` : "Add project"}
        </Button>
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
        "rounded-control focus-visible:ring-ring flex h-14.5 w-full shrink-0 cursor-default items-center gap-3 border px-3 text-left outline-none focus-visible:ring-2",
        chosen ? "bg-accent text-accent-foreground border-border" : "hover:bg-foreground/5 border-transparent",
        added && "opacity-55",
      )}
    >
      <span className="bg-foreground/8 rounded-nav flex size-8.5 shrink-0 items-center justify-center">
        <Folder className={cn("size-icon-md", chosen ? "text-foreground" : "text-foreground/65")} />
      </span>
      <span className="flex min-w-0 flex-1 flex-col gap-0.75">
        <span className="truncate text-sm font-medium">{row.name}</span>
        <span className="text-muted-foreground truncate font-mono text-xs">{shortPath(row.path)}</span>
      </span>
      {added ? (
        <Badge variant="secondary">
          <Check />
          Added
        </Badge>
      ) : (
        <>
          <span className="text-muted-foreground shrink-0 text-xs">
            {row.plain ? "Not a repository" : row.lastUsedMs ? formatLastUsed(row.lastUsedMs, now) : ""}
          </span>
          <span
            aria-hidden
            className={cn(
              "flex size-4.5 shrink-0 items-center justify-center rounded-full",
              chosen ? "bg-toggle-on text-foreground" : "ring-foreground/16 ring-1",
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
        <ErrorLine error={error} />
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
        <ErrorLine error={error} />
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
