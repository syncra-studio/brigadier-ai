import {
  type KeyboardEvent,
  useId,
  useMemo,
  useState,
} from "react";


import { type ResolvedDraft, updateDraft } from "@/app/conversation/draftSetup";

import { PickerRow } from "@/app/conversation/PickerRow";
import { RailPill } from "@/app/conversation/RailPill";
import { MENU, ROW, ROW_ACTIVE } from "@/app/conversation/railMenu";
import { UsageRemaining } from "@/app/conversation/UsageRemaining";

import { Popover, PopoverContent, PopoverTrigger } from "@/components/overlays";

import {
  Branch,
  BranchAlt,
  Check,
  Folder,
  Plus,
  X,
} from "@/components/icons";

import type { Project } from "@/ipc/generated";

import { cn } from "@/lib/utils";

import { select } from "@/state/actions";

import { openAddProject } from "@/state/addProject";

import { useApp } from "@/state/store";
import { SettingsSearch } from "@/components/shared";


/*
 * The utility bar on the composer rail, shown on a new chat only: the project, where the
 * session works, and its branch. Each is a ghost pill (icon and value, no chevron) that opens
 * a menu upward. A session's repository and environment are fixed once it starts, so the bar
 * goes with the first send.
 */

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
          <SettingsSearch
            variant="bordered"
            label="Search projects"
            value={query}
            onChange={setQuery}
            clearable={false}
            clearOnEscape={false}
            role="combobox"
            aria-expanded
            aria-controls={listId}
            autoFocus
            onKeyDown={(event) => onKeyDown(event, (at) => rows[at]?.run())}
            className="mb-1"
          />
          <div
            id={listId}
            role="listbox"
            aria-label="Projects"
            className="max-h-command-list overflow-y-auto"
          >
            {shown.map((entry) => (
              <PickerRow
                key={entry.id}
                icon={<Folder />}
                title={entry.name}
                selected={entry.id === project?.id}
                highlighted={highlighted === entry.id}
                onPointerMove={() => setIndex(rows.findIndex((row) => row.key === entry.id))}
                onClick={() => choose(entry.id)}
              />
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

/**
 * The "Work in" menu: where the session works, always a new worktree of its own (only migrated
 * sessions work in the local checkout), and the usage left.
 */
export function WorkInMenu({ resolved }: { resolved: ResolvedDraft }) {
  const [open, setOpen] = useState(false);
  const repoName = resolved.repo.info?.name ?? resolved.project?.name ?? "the repository";
  const tip = `Every tab starts clean on a copy of ${repoName} of its own, to work in parallel.`;
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <RailPill icon={<BranchAlt />} label="Work in" title={tip}>
          New worktree
        </RailPill>
      </PopoverTrigger>
      <PopoverContent side="top" align="start" className={cn(MENU, "w-2xs")}>
        <p className="text-muted-foreground px-2 py-1 text-sm">Work in</p>
        <p title={tip} className={ROW}>
          <BranchAlt />
          <span className="flex-1">New worktree</span>
          <Check />
        </p>
        <div className="border-foreground/10 mt-1 border-t pt-1">
          <UsageRemaining provider={resolved.model.provider} />
        </div>
      </PopoverContent>
    </Popover>
  );
}

// ----- branch ----------------------------------------------------------------------------

/**
 * The base branch popover: a search (not focused on open), the branches (checked-out first, ✓
 * on the pick) in a fixed-height list. The session's worktree starts from the pick.
 */
export function BranchPopover({ resolved }: { resolved: ResolvedDraft }) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const listId = useId();
  const projectId = resolved.project?.id ?? null;
  const { info, error, loading } = resolved.repo;
  const picked = resolved.base;
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
    updateDraft(projectId, { base: name });
    setOpen(false);
  };
  const { index, setIndex, onKeyDown } = useHighlight(matches.length, needle);

  return (
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
          label="Base branch"
          title="What branch should this session start from?"
        >
          {picked ?? (loading ? "…" : "Pick a branch")}
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
        <SettingsSearch
          variant="bordered"
          label={`Search ${repoName} branches`}
          value={query}
          onChange={setQuery}
          clearable={false}
          clearOnEscape={false}
          role="combobox"
          aria-expanded
          aria-controls={listId}
          className="mb-1"
        />
        <p className="text-muted-foreground px-2 py-1 text-sm">Local branches</p>
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
              {name === picked && <Check />}
            </button>
          ))}
        </div>
      </PopoverContent>
    </Popover>
  );
}
