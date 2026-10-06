import { useAui } from "@assistant-ui/react";
import {
  Branch,
  BranchAlt,
  Check,
  ChevronDown,
  ChevronRight,
  Desktop,
  Folder,
  MagnifyingGlassSearch,
  Plus,
  SpeedometerLatencySpeed,
  X,
} from "@openai/apps-sdk-ui/components/Icon";
import {
  type FormEvent,
  forwardRef,
  type KeyboardEvent,
  type ReactNode,
  useEffect,
  useId,
  useMemo,
  useState,
} from "react";

import { type ResolvedDraft, updateDraft } from "@/app/conversation/draftSetup";
import { resetsAt, windowName } from "@/app/conversation/StatusCard";
import { NameDialog } from "@/app/NameDialog";
import { composerPill } from "@/components/assistant-ui/elements/surfaces";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import type { EnvironmentKind, Project, QuotaWindow, RepoInfo } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { loadProviders, select, updateProject } from "@/state/actions";
import { openAddProject } from "@/state/addProject";
import { useApp } from "@/state/store";

/*
 * The utility bar on the composer rail, shown on a new chat only: the project, where the
 * session works, and its branch. Each is a ghost pill (icon and value, no chevron) that opens
 * a menu upward. A session's repository and environment are fixed once it starts, so the bar
 * goes with the first send.
 */

/** What "Create branch for this session" suggests before a name, unless the project says. */
const DEFAULT_BRANCH_PREFIX = "brigadier/";

/** The menu surface, shared by the rail's menus. */
const MENU = "rounded-menu p-1";
const ROW =
  "rounded-capsule min-h-control-sm flex w-full items-center gap-2 px-2 text-start text-sm outline-none transition-colors [&_svg]:size-icon-md [&_svg]:shrink-0";
const ROW_ACTIVE = "bg-foreground/8";

/** A rail trigger: the composer's pill, in the rail's bright text. */
const RailPill = forwardRef<
  HTMLButtonElement,
  { icon: ReactNode; label: string; title?: string; children: ReactNode }
>(({ icon, label, title, children, ...props }, ref) => (
  <button
    ref={ref}
    type="button"
    aria-label={label}
    title={title}
    className={cn(composerPill, "text-foreground max-w-xs")}
    {...props}
  >
    {icon}
    <span className="truncate">{children}</span>
  </button>
));
RailPill.displayName = "RailPill";

/** ↑/↓ over a list of rows, Enter picks the highlighted one. */
function useHighlight(count: number, query: string) {
  // A new query highlights the first row again.
  const [state, setState] = useState({ query, index: 0 });
  const index = state.query === query ? state.index : 0;
  const setIndex = (next: number | ((current: number) => number)) =>
    setState({ query, index: typeof next === "function" ? next(index) : next });
  const onKeyDown = (event: KeyboardEvent, pick: (index: number) => void) => {
    if (count === 0) return;
    if (event.key === "ArrowDown") {
      event.preventDefault();
      setIndex((current) => (current + 1) % count);
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      setIndex((current) => (current - 1 + count) % count);
    } else if (event.key === "Enter") {
      event.preventDefault();
      pick(Math.min(index, count - 1));
    }
  };
  return { index, setIndex, onKeyDown };
}

function SearchField({
  value,
  onChange,
  placeholder,
  autoFocus,
  onKeyDown,
  controls,
}: {
  controls: string;
  value: string;
  onChange: (value: string) => void;
  placeholder: string;
  autoFocus: boolean;
  onKeyDown?: (event: KeyboardEvent<HTMLInputElement>) => void;
}) {
  return (
    <div className="border-foreground/10 mb-1 flex items-center gap-2 border-b px-2 pb-1">
      <MagnifyingGlassSearch aria-hidden className="text-muted-foreground size-icon-md shrink-0" />
      <input
        role="combobox"
        aria-expanded
        aria-controls={controls}
        aria-label={placeholder}
        value={value}
        placeholder={placeholder}
        // oxlint-disable-next-line jsx-a11y/no-autofocus
        autoFocus={autoFocus}
        onChange={(event) => onChange(event.target.value)}
        onKeyDown={onKeyDown}
        className="placeholder:text-muted-foreground h-control-md min-w-0 flex-1 bg-transparent text-sm outline-none"
      />
    </div>
  );
}

// ----- project ---------------------------------------------------------------------------

type ProjectRow = { key: string; run: () => void };

/**
 * The project picker: "Search projects", the projects (✓ on the one in use), "Add project"
 * (a typed path opens it on that folder) and "Don't work in a project" (a Chat). With no project the pill reads "Choose project".
 */
export function ProjectCombobox({
  project,
  inHeading = false,
}: {
  project: Project | null;
  /** The hero's "What should we build in {project}?": the name is the trigger. */
  inHeading?: boolean;
}) {
  const projects = useApp((s) => s.projects);
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const listId = useId();
  const sorted = useMemo(
    () => Object.values(projects).toSorted((a, b) => a.name.localeCompare(b.name)),
    [projects],
  );
  const needle = query.trim().toLowerCase();
  // A typed path is a folder to add: "Add ~/code/app".
  const typedPath = /^(~|\/|[A-Za-z]:[\\/])/.test(query.trim()) ? query.trim() : "";
  const shown = sorted.filter((entry) => entry.name.toLowerCase().includes(needle));
  const choose = (id: string | null) => {
    setOpen(false);
    // The same draft, for another project: no new place to go back from.
    select(
      id ? { type: "draft", kind: "session", projectId: id } : { type: "draft", kind: "chat" },
      "replace",
    );
  };
  const rows: ProjectRow[] = [
    ...shown.map((entry) => ({ key: entry.id, run: () => choose(entry.id) })),
    {
      key: "new",
      run: () => {
        setOpen(false);
        openAddProject(typedPath);
      },
    },
    ...(project ? [{ key: "none", run: () => choose(null) }] : []),
  ];
  const { index, setIndex, onKeyDown } = useHighlight(rows.length, needle);
  const highlighted = rows[index]?.key;

  return (
    <>
      <Popover
        open={open}
        onOpenChange={(next) => {
          setOpen(next);
          if (!next) setQuery("");
        }}
      >
        <PopoverTrigger asChild>
          {inHeading ? (
            <button
              type="button"
              aria-label="Project"
              className="decoration-muted-foreground hover:text-muted-foreground cursor-pointer underline decoration-dotted underline-offset-4 transition-colors"
            >
              {project?.name}
            </button>
          ) : (
            <RailPill
              icon={<Folder />}
              label="Project"
              title={
                project ? "Change the project for this chat" : "Select a project to run your chat in"
              }
            >
              {project ? project.name : "Choose project"}
            </RailPill>
          )}
        </PopoverTrigger>
        <PopoverContent
          side={inHeading ? "bottom" : "top"}
          align={inHeading ? "center" : "start"}
          className={cn(MENU, "w-xs")}
        >
          <SearchField
            controls={listId}
            value={query}
            onChange={setQuery}
            placeholder="Search projects"
            autoFocus
            onKeyDown={(event) => onKeyDown(event, (at) => rows[at]?.run())}
          />
          <div
            id={listId}
            role="listbox"
            aria-label="Projects"
            className="max-h-command-list overflow-y-auto"
          >
            {shown.map((entry) => (
              <button
                key={entry.id}
                type="button"
                role="option"
                aria-selected={entry.id === project?.id}
                onPointerMove={() => setIndex(rows.findIndex((row) => row.key === entry.id))}
                onClick={() => choose(entry.id)}
                className={cn(ROW, highlighted === entry.id && ROW_ACTIVE)}
              >
                <Folder className="text-muted-foreground" />
                <span className="min-w-0 flex-1 truncate">{entry.name}</span>
                {entry.id === project?.id && <Check />}
              </button>
            ))}
          </div>
          {shown.length === 0 && !typedPath && (
            <p className="text-muted-foreground px-2 py-1.5 text-sm">No projects found</p>
          )}
          <div className="border-foreground/10 mt-1 border-t pt-1">
            <button
              type="button"
              onPointerMove={() => setIndex(rows.findIndex((row) => row.key === "new"))}
              onClick={() => rows.find((row) => row.key === "new")?.run()}
              className={cn(ROW, highlighted === "new" && ROW_ACTIVE)}
            >
              <Plus />
              <span className="min-w-0 flex-1 truncate">
                {typedPath ? `Add ${typedPath}` : "Add project"}
              </span>
            </button>
            {project && (
              <button
                type="button"
                onPointerMove={() => setIndex(rows.findIndex((row) => row.key === "none"))}
                onClick={() => choose(null)}
                className={cn(ROW, highlighted === "none" && ROW_ACTIVE)}
              >
                <X />
                Don't work in a project
              </button>
            )}
          </div>
        </PopoverContent>
      </Popover>
    </>
  );
}

// ----- where the session works -----------------------------------------------------------

const PLACES: Record<EnvironmentKind, { label: string; icon: ReactNode }> = {
  localCheckout: { label: "Local checkout", icon: <Desktop /> },
  newWorktree: { label: "New worktree", icon: <BranchAlt /> },
};

/** A usage window's name: "Weekly", "5h". */
function usageName(window: QuotaWindow): string {
  return window.windowMinutes === 10080 ? "Weekly" : windowName(window);
}

/** "Usage remaining ›": the model's provider's windows, expanded in place. */
function UsageRemaining({ provider }: { provider: string }) {
  const [open, setOpen] = useState(false);
  const view = useApp((s) => s.providers.view);
  useEffect(() => {
    if (open && !view) void loadProviders().catch(() => {});
  }, [open, view]);
  const windows =
    view?.providers.find((overview) => overview.provider === provider)?.quota?.windows ?? [];
  return (
    <>
      <button
        type="button"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
        className={cn(ROW, "hover:bg-foreground/8")}
      >
        <SpeedometerLatencySpeed />
        <span className="flex-1">Usage remaining</span>
        {open ? <ChevronDown /> : <ChevronRight />}
      </button>
      {open && (
        <div className="flex flex-col gap-1 px-2 pb-1 ps-8 text-sm">
          {windows.length === 0 && (
            <p className="text-muted-foreground">{view ? "Not reported yet" : "Loading…"}</p>
          )}
          {windows.map((window) => (
            <p key={window.id} className="flex items-center gap-2">
              <span className="flex-1">{usageName(window)}</span>
              <span className="tabular-nums">
                {Math.max(0, Math.round(100 - window.usedPercent))}%
              </span>
              {window.resetsAtMs !== null && (
                <span className="text-muted-foreground">{resetsAt(window.resetsAtMs, window)}</span>
              )}
            </p>
          ))}
        </div>
      )}
    </>
  );
}

/** The "Work in" menu: the local checkout or a new worktree, and the usage left. */
export function WorkInMenu({ resolved }: { resolved: ResolvedDraft }) {
  const [open, setOpen] = useState(false);
  const projectId = resolved.project?.id ?? null;
  const repoName = resolved.repo.info?.name ?? resolved.project?.name ?? "the repository";
  const place = PLACES[resolved.environment];
  const tips: Record<EnvironmentKind, string> = {
    localCheckout: `Work lands on ${resolved.branch ?? "the branch you pick"}`,
    newWorktree: `Create a copy of ${repoName} to work in parallel.`,
  };
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <RailPill icon={place.icon} label="Work in" title={tips[resolved.environment]}>
          {place.label}
        </RailPill>
      </PopoverTrigger>
      <PopoverContent side="top" align="start" className={cn(MENU, "w-2xs")}>
        <p className="text-muted-foreground px-2 py-1 text-sm">Work in</p>
        {(Object.keys(PLACES) as EnvironmentKind[]).map((kind) => (
          <button
            key={kind}
            type="button"
            role="menuitemradio"
            aria-checked={resolved.environment === kind}
            title={tips[kind]}
            onClick={() => {
              updateDraft(projectId, { environment: kind });
              setOpen(false);
            }}
            className={cn(ROW, "hover:bg-foreground/8")}
          >
            {PLACES[kind].icon}
            <span className="flex-1">{PLACES[kind].label}</span>
            {resolved.environment === kind && <Check />}
          </button>
        ))}
        <div className="border-foreground/10 mt-1 border-t pt-1">
          <UsageRemaining provider={resolved.model.provider} />
        </div>
      </PopoverContent>
    </Popover>
  );
}

// ----- branch ----------------------------------------------------------------------------

/** Why `name` can't be a new branch, in git's terms, or `null`. */
function branchProblem(name: string, info: RepoInfo | null): string | null {
  if (!name) return "Enter a branch name.";
  if (name.endsWith("/")) return 'Branch name cannot end with "/".';
  if (name.startsWith("/") || name.startsWith("-")) return `Branch name cannot start with "${name[0]}".`;
  if (name.endsWith(".") || name.endsWith(".lock")) return "Branch name cannot end with \".\" or \".lock\".";
  if (/\s/.test(name)) return "Branch name cannot contain spaces.";
  if (name.includes("..") || name.includes("//") || name.includes("@{"))
    return 'Branch name cannot contain "..", "//" or "@{".';
  if (/[~^:?*[\\]/.test(name)) return "Branch name contains a character git doesn't allow.";
  if (name.split("/").some((part) => part.startsWith(".")))
    return 'A part of the name cannot start with ".".';
  if (info?.branches.some((branch) => branch.name === name)) return "A branch with this name already exists.";
  return null;
}

/** Words of the message being typed, as a branch name ("fix the login bug" → "fix-the-login-bug"). */
function slug(text: string): string {
  return text
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .split("-")
    .slice(0, 5)
    .join("-");
}

/**
 * "Create branch for this session": the create dialog, saying what Brigadier does. The
 * branch is created from the picked one when the session starts; the user's checkout stays
 * on its branch.
 */
type CreateBranchProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  resolved: ResolvedDraft;
  from: string;
};

function CreateBranchDialog(props: CreateBranchProps) {
  // Remounted per opening, so the name is suggested afresh.
  return props.open ? <CreateBranchForm {...props} /> : null;
}

function CreateBranchForm({ open, onOpenChange, resolved, from }: CreateBranchProps) {
  const aui = useAui();
  const project = resolved.project;
  const prefix = project?.prefs.branchPrefix ?? DEFAULT_BRANCH_PREFIX;
  const [name, setName] = useState(
    () => resolved.draft.newBranch ?? `${prefix}${slug(aui.composer().getState().text)}`,
  );
  const [editingPrefix, setEditingPrefix] = useState(false);
  const problem = branchProblem(name.trim(), resolved.repo.info);
  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (problem) return;
    updateDraft(project?.id ?? null, { branch: from, newBranch: name.trim() });
    onOpenChange(false);
  };
  return (
    <>
      <Dialog open={open} onOpenChange={onOpenChange}>
        <DialogContent className="max-w-sm">
          <form onSubmit={submit} className="grid gap-4">
            <DialogHeader>
              <DialogTitle>Create branch for this session</DialogTitle>
            </DialogHeader>
            <div className="grid gap-1.5">
              <div className="flex items-center justify-between text-sm">
                <label htmlFor="new-branch-name">Branch name</label>
                {project && (
                  <button
                    type="button"
                    onClick={() => setEditingPrefix(true)}
                    className="text-muted-foreground hover:text-foreground transition-colors"
                  >
                    Set prefix
                  </button>
                )}
              </div>
              <Input
                id="new-branch-name"
                value={name}
                onChange={(event) => setName(event.target.value)}
                className="font-mono"
              />
              {name && problem ? (
                <p role="alert" className="text-destructive text-xs">
                  {problem}
                </p>
              ) : (
                <p className="text-muted-foreground text-xs">
                  Created from {from} when the session starts; work lands on it. Your checkout
                  stays on its branch.
                </p>
              )}
            </div>
            <DialogFooter>
              <Button type="button" variant="ghost" onClick={() => onOpenChange(false)}>
                Close
              </Button>
              <Button type="submit" disabled={problem !== null}>
                Create branch
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>
      {project && (
        <NameDialog
          open={editingPrefix}
          onOpenChange={setEditingPrefix}
          title="Branch prefix"
          description={`Put before the names suggested for ${project.name}'s new branches.`}
          label="Prefix"
          initialValue={prefix}
          confirmLabel="Save"
          onSubmit={async (value) => {
            await updateProject(project.id, {
              name: null,
              repos: null,
              prefs: { ...project.prefs, branchPrefix: value },
            });
            setName((current) => `${value}${current.startsWith(prefix) ? current.slice(prefix.length) : current}`);
          }}
        />
      )}
    </>
  );
}

/**
 * The branch popover: a search (not focused on open), the branches (default and checked-out
 * first, ✓ on the pick) in a fixed-height list, then "Create branch for this session…". In a
 * new worktree it picks the branch the session starts from.
 */
export function BranchPopover({ resolved }: { resolved: ResolvedDraft }) {
  const [open, setOpen] = useState(false);
  const [creating, setCreating] = useState(false);
  const [query, setQuery] = useState("");
  const listId = useId();
  const projectId = resolved.project?.id ?? null;
  const { info, error, loading } = resolved.repo;
  const local = resolved.environment === "localCheckout";
  const picked = local ? (resolved.draft.branch ?? info?.currentBranch ?? null) : resolved.base;
  const shown = local ? (resolved.draft.newBranch ?? picked) : picked;
  const repoName = info?.name ?? resolved.project?.name ?? "";

  const branches = useMemo(() => {
    if (!info) return [];
    const current = info.currentBranch;
    // The checked-out branch first, then the rest by name.
    return info.branches
      .map((branch) => branch.name)
      .toSorted((a, b) => Number(b === current) - Number(a === current) || a.localeCompare(b));
  }, [info]);
  const needle = query.trim().toLowerCase();
  const matches = branches.filter((name) => name.toLowerCase().includes(needle));
  const pick = (name: string) => {
    updateDraft(projectId, local ? { branch: name, newBranch: null } : { base: name });
    setOpen(false);
  };
  const { index, setIndex, onKeyDown } = useHighlight(matches.length, needle);

  return (
    <>
      <Popover
        open={open}
        onOpenChange={(next) => {
          setOpen(next);
          if (!next) setQuery("");
        }}
      >
        <PopoverTrigger asChild>
          <RailPill
            icon={<Branch />}
            label={local ? "Branch" : "Base branch"}
            title={local ? "The branch work lands on" : "What branch should this session start from?"}
          >
            {shown ?? (loading ? "…" : "Pick a branch")}
          </RailPill>
        </PopoverTrigger>
        <PopoverContent
          side="top"
          align="start"
          className={cn(MENU, "w-xs")}
          // Leaves the search unfocused: focus stays on the menu, where ↑/↓ still move.
          onOpenAutoFocus={(event) => {
            event.preventDefault();
            (event.currentTarget as HTMLElement | null)?.focus();
          }}
          onKeyDown={(event) => {
            // From the menu itself or its search; a focused row's own Enter stays its own.
            if (event.target !== event.currentTarget && !(event.target instanceof HTMLInputElement)) return;
            onKeyDown(event, (at) => matches[at] && pick(matches[at]));
          }}
        >
          <SearchField
            controls={listId}
            value={query}
            onChange={setQuery}
            placeholder={`Search ${repoName} branches`}
            autoFocus={false}
          />
          <p className="text-muted-foreground px-2 py-1 text-sm">
            {local ? "Branches" : "Local branches"}
          </p>
          <div id={listId} role="listbox" aria-label="Branches" className="h-command-list overflow-y-auto">
            {error && (
              <p role="alert" className="text-destructive px-2 py-1 text-xs">
                Branches unavailable: {error}
              </p>
            )}
            {!info && loading && (
              <p className="text-muted-foreground px-2 py-1 text-sm">Reading the repository…</p>
            )}
            {info && matches.length === 0 && (
              <p className="text-muted-foreground px-2 py-1 text-sm">No branches found</p>
            )}
            {matches.map((name, at) => (
              <button
                key={name}
                type="button"
                role="option"
                aria-selected={name === picked}
                onPointerMove={() => setIndex(at)}
                onClick={() => pick(name)}
                className={cn(ROW, at === index && ROW_ACTIVE)}
              >
                <Branch className="text-muted-foreground" />
                <span className="min-w-0 flex-1 truncate">{name}</span>
                {name === picked && !(local && resolved.draft.newBranch) && <Check />}
              </button>
            ))}
          </div>
          {local && (
            <div className="border-foreground/10 mt-1 border-t pt-1">
              {resolved.draft.newBranch && (
                <button
                  type="button"
                  onClick={() => updateDraft(projectId, { newBranch: null })}
                  className={cn(ROW, "hover:bg-foreground/8")}
                >
                  <X />
                  <span className="min-w-0 flex-1 truncate">
                    Don't create {resolved.draft.newBranch}
                  </span>
                </button>
              )}
              <button
                type="button"
                disabled={!picked}
                onClick={() => {
                  setOpen(false);
                  setCreating(true);
                }}
                className={cn(ROW, "hover:bg-foreground/10 disabled:opacity-50")}
              >
                <Plus />
                Create branch for this session…
              </button>
            </div>
          )}
        </PopoverContent>
      </Popover>
      {picked && (
        <CreateBranchDialog
          open={creating}
          onOpenChange={setCreating}
          resolved={resolved}
          from={picked}
        />
      )}
    </>
  );
}
