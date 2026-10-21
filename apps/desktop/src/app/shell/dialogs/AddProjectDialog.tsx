import {
  useEffect,
  useEffectEvent,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
  type RefObject,
} from "react";

import { CancelButton, DEFAULT_PARENT, Footer, Header } from "@/app/shell/dialogs/addProjectParts";
import { CloneView } from "@/app/shell/dialogs/CloneView";
import { errorText, PROJECT_DIALOG_CARD } from "@/app/shell/dialogs/fields";
import { NewRepoView } from "@/app/shell/dialogs/NewRepoView";
import { SettingsSearch } from "@/components/shared";
import { Check, Folder, FolderOpen, Link, Plus, Spinner } from "@/components/icons";
import { Badge, Button, ErrorLine } from "@/components/controls";
import { Dialog, DialogContent } from "@/components/overlays";
import { useNow } from "@/hooks/use-now";
import { pickFolder } from "@/ipc/client";
import type { FolderCheck, RepoSuggestion } from "@/ipc/generated";
import { formatLastUsed, shortPath } from "@/lib/format";
import { cn } from "@/lib/utils";
import {
  addProject,
  checkFolder,
  closeAddProject,
  findRepos,
  showProject,
  useAddProject,
} from "@/state/addProject";

/*
 * Add project, laid out to the user's design and drawn with the app's dialog parts: pick a git
 * repository found on this computer (or choose a folder), or clone one from a URL, or start a
 * new one.
 */

export function AddProjectDialog() {
  const open = useAddProject((s) => s.open);
  const path = useAddProject((s) => s.path);
  const opening = useAddProject((s) => s.opening);
  return (
    <Dialog open={open} onOpenChange={(next) => !next && closeAddProject()}>
      <DialogContent
        data-testid="add-project-dialog"
        showCloseButton={false}
        className={cn(
          PROJECT_DIALOG_CARD,
          "max-w-add-project top-add-project-top origin-top translate-y-0 gap-0 p-0 motion-reduce:animate-none",
        )}
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
          className="h-9 [&>svg]:size-icon-sm"
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
          <span className="text-muted-foreground text-xs leading-3.75">On this Mac</span>
          <span className="text-muted-foreground text-xs leading-3.75">Most recent first</span>
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
            <p className="text-muted-foreground flex h-14.5 items-center px-3 text-sm leading-4.25">
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
            <ErrorLine error={error} className="leading-4.25" />
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

/** A repository in the list: its mark, name, path, last use (or Added) and its radio. */
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
        "rounded-surface focus-visible:outline-ring flex h-14.5 w-full shrink-0 cursor-default items-center gap-3 border px-3 text-left outline-none focus-visible:outline-solid focus-visible:outline-2 focus-visible:-outline-offset-2",
        chosen ? "bg-accent text-accent-foreground border-border" : "hover:bg-foreground/5 border-transparent",
        added && "opacity-55",
      )}
    >
      <span className="bg-foreground/8 rounded-lg flex size-8.5 shrink-0 items-center justify-center">
        <Folder className={cn("size-icon-md", chosen ? "text-foreground" : "text-foreground/65")} />
      </span>
      <span className="flex min-w-0 flex-1 flex-col gap-0.75">
        <span className="truncate text-sm leading-4.25 font-medium">{row.name}</span>
        <span className="text-muted-foreground truncate font-mono text-xs">{shortPath(row.path)}</span>
      </span>
      {added ? (
        <Badge variant="secondary">
          <Check />
          Added
        </Badge>
      ) : (
        <span className="text-muted-foreground shrink-0 text-xs">
          {row.plain ? "Not a repository" : row.lastUsedMs ? formatLastUsed(row.lastUsedMs, now) : ""}
        </span>
      )}
      <span
        aria-hidden
        className={cn(
          "flex size-4.5 shrink-0 items-center justify-center rounded-full",
          chosen ? "bg-toggle-on text-foreground" : "border-foreground/16 border",
        )}
      >
        {chosen && <Check className="size-icon-xs" />}
      </span>
    </button>
  );
}
