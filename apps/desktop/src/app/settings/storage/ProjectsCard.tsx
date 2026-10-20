import { useState, type ReactNode } from "react";

import {
  PROJECTS_PAGE,
  PROJECT_SORTS,
  TOP_PROJECTS,
  canGoBytes,
  conversationsLine,
  count,
  isUnused,
  listProjects,
  projectParts,
  sharedParts,
  size,
  unusedProjects,
  usedWords,
  type Part,
  type ProjectSort,
} from "@/app/settings/storage/model";
import { Dot, RoundBadge, ShareBar, StorageCard } from "@/app/settings/storage/parts";
import { selectTrigger } from "@/app/settings/parts";
import {
  Check,
  ChevronDown,
  ChevronRight,
  ChevronUp,
  Folder,
  FolderOpen,
  Folders,
  History,
  Search,
  Sparkles,
  Spinner,
  Stack,
} from "@/components/icons";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { revealPath } from "@/ipc/client";
import type { ProjectUsage, StorageReport } from "@/ipc/generated";
import { shortPath } from "@/lib/format";
import { cn } from "@/lib/utils";

/** How freeing up the projects not in use goes. */
export type UnusedFreeUp =
  | { phase: "idle" }
  | { phase: "confirm" }
  | { phase: "freeing" }
  | { phase: "done"; title: string; line: string }
  | { phase: "failed"; line: string };

/**
 * Disk use by project: the largest four, the rest in one row, and what all projects share;
 * opening one shows what's in it with a cleanup for just that project. Show all opens the full
 * list in the same card: search, sort, the projects not used for three months, 20 at a time.
 */
export function ProjectsCard({
  report,
  nowMs,
  reveal,
  unused,
  onUnused,
  onFreeUpUnused,
  onFreeUpProject,
  cleaning,
}: {
  report: StorageReport;
  nowMs: number;
  /** "Reveal in Finder" (or "Show in folder"). */
  reveal: string;
  unused: UnusedFreeUp;
  onUnused: (state: UnusedFreeUp) => void;
  onFreeUpUnused: (ids: string[]) => void;
  /** Reviews what of a project can go, in the Free up space card. */
  onFreeUpProject: (projectId: string) => void;
  /** A clean runs: a project's Free up waits for it. */
  cleaning: boolean;
}) {
  const [all, setAll] = useState(false);
  const [opened, setOpened] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [sort, setSort] = useState<ProjectSort>("largest");
  const [shown, setShown] = useState(PROJECTS_PAGE);
  const total = report.totalBytes;
  const toggle = (id: string) => setOpened(opened === id ? null : id);

  if (all) {
    const found = listProjects(report.projects, sort, query);
    const stale = unusedProjects(report, nowMs);
    return (
      <StorageCard data-projects="all">
        <div className="border-divider flex items-center gap-3 border-b py-3.5">
          <label className="rounded-capsule border-foreground/16 bg-foreground/8 flex h-8 min-w-0 flex-1 items-center gap-2 border px-2.5">
            <Search aria-hidden className="text-foreground/65 size-icon-md shrink-0" />
            <input
              value={query}
              onChange={(event) => {
                setQuery(event.target.value);
                setShown(PROJECTS_PAGE);
              }}
              placeholder={`Search ${count(report.projects.length, "project", "projects")}`}
              aria-label="Search projects"
              className="placeholder:text-foreground/50 min-w-0 flex-1 bg-transparent text-sm outline-none"
            />
          </label>
          <SortSelect value={sort} onChange={setSort} />
          <div className="ps-1">
            <Button
              variant="link-blue"
              size="inline"
              onClick={() => {
                setAll(false);
                setQuery("");
                setShown(PROJECTS_PAGE);
              }}
            >
              Show fewer
                <ChevronUp aria-hidden />
            </Button>
          </div>
        </div>
        {(stale.projects.length > 0 || unused.phase === "done" || unused.phase === "failed") && (
          <UnusedRow
            projects={stale.projects}
            bytes={stale.bytes}
            state={unused}
            onState={onUnused}
            onFreeUp={() => onFreeUpUnused(stale.projects.map((project) => project.projectId))}
          />
        )}
        {found.slice(0, shown).map((project) => (
          <ProjectRows
            key={project.projectId}
            project={project}
            report={report}
            compact
            nowMs={nowMs}
            open={opened === project.projectId}
            onToggle={() => toggle(project.projectId)}
            reveal={reveal}
            onFreeUp={onFreeUpProject}
            cleaning={cleaning}
          />
        ))}
        <div className="flex items-center justify-between py-3.5">
          <span className="text-muted-foreground text-xs">
            {found.length === 0 ? "No project matches" : `Showing ${Math.min(shown, found.length)} of ${found.length}`}
          </span>
          {found.length > shown && (
            <Button variant="link-blue" size="inline" onClick={() => setShown(shown + PROJECTS_PAGE)}>
              Show {Math.min(PROJECTS_PAGE, found.length - shown)} more
              <ChevronDown aria-hidden />
            </Button>
          )}
        </div>
      </StorageCard>
    );
  }

  const ordered = listProjects(report.projects, "largest");
  const top = ordered.slice(0, TOP_PROJECTS);
  const rest = ordered.slice(TOP_PROJECTS);
  const restParts = sumParts(rest.map(projectParts));
  const restUnused = rest.filter((project) => isUnused(project, nowMs)).length;
  const shared = sharedParts(report);
  return (
    <StorageCard data-projects="top" className="divide-divider divide-y">
      {top.map((project) => (
        <ProjectRows
          key={project.projectId}
          project={project}
          report={report}
          nowMs={nowMs}
          open={opened === project.projectId}
          onToggle={() => toggle(project.projectId)}
          reveal={reveal}
          onFreeUp={onFreeUpProject}
          cleaning={cleaning}
        />
      ))}
      {rest.length > 0 && (
        <div data-project="more" className="flex items-center gap-4 py-3.5">
          <Folders aria-hidden className="text-foreground/65 size-4 shrink-0" />
          <div className="flex min-w-0 flex-1 flex-col gap-1">
            <span className="text-foreground text-label font-medium">{count(rest.length, "more project", "more projects")}</span>
            {restUnused > 0 && (
              <span className="text-muted-foreground text-xs">
                {rest.length === 1
                  ? "It hasn't been used in 3 months"
                  : `${restUnused === rest.length ? "None" : restUnused} of them ${restUnused === rest.length ? "have been used in 3 months" : restUnused === 1 ? "hasn't been used in 3 months" : "haven't been used in 3 months"}`}
              </span>
            )}
          </div>
          <ShareBar parts={restParts} total={total} />
          <Size bytes={sumBytes(restParts)} />
          <Button variant="link-blue" size="inline" onClick={() => setAll(true)}>
            Show all
            <ChevronRight aria-hidden />
          </Button>
        </div>
      )}
      <ExpandableRow
        name="shared"
        icon={<Stack aria-hidden className="text-foreground/65 size-4 shrink-0" />}
        title={<span className="text-foreground text-label font-medium">Shared by all projects</span>}
        line="Models, logs & recordings, and other"
        parts={shared}
        total={total}
        open={opened === "shared"}
        onToggle={() => toggle("shared")}
      >
        <DetailRow part={shared[0]!} line="Speech and code search models Brigadier downloaded" />
        <DetailRow part={shared[1]!} line="What the app and its agents wrote down, and voice recordings" />
        <DetailRow part={shared[2]!} line="The database, settings and everything no project holds" />
      </ExpandableRow>
    </StorageCard>
  );
}

/** A project's row and, open, what's in it with its own cleanup. */
function ProjectRows({
  project,
  report,
  compact = false,
  nowMs,
  open,
  onToggle,
  reveal,
  onFreeUp,
  cleaning,
}: {
  project: ProjectUsage;
  report: StorageReport;
  /** The full list's one-line row: name and when it was last used. */
  compact?: boolean;
  nowMs: number;
  open: boolean;
  onToggle: () => void;
  reveal: string;
  onFreeUp: (projectId: string) => void;
  cleaning: boolean;
}) {
  const parts = projectParts(project);
  const [brains, conversations, work] = parts as [Part, Part, Part];
  const canGo = canGoBytes(report, project.projectId);
  const items = report.items.filter(
    (item) => item.projectId === project.projectId && item.checked && item.selectable,
  ).length;
  const leftover = report.items.filter(
    (item) => item.projectId === project.projectId && item.category === "finishedWork" && item.selectable,
  ).length;
  const name = <span className="text-foreground font-mono text-xs font-medium">{project.name}</span>;
  return (
    <ExpandableRow
      name={project.projectId}
      compact={compact}
      icon={<Folder aria-hidden className="text-foreground/65 size-4 shrink-0" />}
      title={
        compact ? (
          <span className="flex min-w-0 items-center gap-2.5">
            {name}
            <span
              className={cn(
                "shrink-0 text-xs",
                isUnused(project, nowMs) ? "text-warning" : "text-muted-foreground",
              )}
            >
              {usedWords(project.lastUsedMs, nowMs)}
            </span>
          </span>
        ) : (
          name
        )
      }
      line={compact ? undefined : project.path ? shortPath(project.path) : "No folder"}
      parts={parts}
      total={report.totalBytes}
      open={open}
      onToggle={onToggle}
    >
      <DetailRow part={brains} line="What agents have learned about this project" />
      <DetailRow part={conversations} line={conversationsLine(project)} />
      <DetailRow
        part={work}
        line={
          leftover > 0
            ? `${count(leftover, "left over from a finished session", "left over from finished sessions")}`
            : work.bytes > 0
              ? "Used by its open sessions"
              : "None left over from finished sessions"
        }
      />
      <div className="border-divider flex items-center gap-2 border-t pt-3">
        <span className="text-foreground/65 min-w-0 flex-1 text-xs">
          {items === 0
            ? "Nothing of this project's can go"
            : canGo > 0
              ? `${size(canGo)} of this project's files can go`
              : `${count(items, "small leftover", "small leftovers")} of this project's can go`}
        </span>
        {project.path && (
          <Button variant="soft" size="sm" onClick={() => void revealPath(project.path!)}>
            <FolderOpen aria-hidden />
            {reveal}
          </Button>
        )}
        {items > 0 && (
          <Button variant="soft" size="sm" disabled={cleaning} onClick={() => onFreeUp(project.projectId)}>
            <Sparkles aria-hidden />
            {canGo > 0 ? `Free up ${size(canGo)}` : "Tidy up"}
          </Button>
        )}
      </div>
    </ExpandableRow>
  );
}

/** A row that opens: icon, name (and line), share bar, size, chevron; open, its detail. */
function ExpandableRow({
  name,
  compact = false,
  icon,
  title,
  line,
  parts,
  total,
  open,
  onToggle,
  children,
}: {
  name: string;
  compact?: boolean;
  icon: ReactNode;
  title: ReactNode;
  line?: string | undefined;
  parts: Part[];
  total: number;
  open: boolean;
  onToggle: () => void;
  children: ReactNode;
}) {
  return (
    <div data-project={name} className={cn("flex flex-col", compact && "border-divider border-b")}>
      <button
        type="button"
        aria-expanded={open}
        onClick={onToggle}
        className={cn("flex w-full items-center gap-4 text-left", compact ? "h-12" : "py-3.5")}
      >
        {icon}
        <span className="flex min-w-0 flex-1 flex-col gap-1">
          {title}
          {line && <span className="text-muted-foreground truncate text-xs">{line}</span>}
        </span>
        <ShareBar parts={parts} total={total} />
        <Size bytes={sumBytes(parts)} />
        {open ? (
          <ChevronDown aria-hidden className="text-muted-foreground size-3.75 shrink-0" />
        ) : (
          <ChevronRight aria-hidden className="text-muted-foreground size-3.75 shrink-0" />
        )}
      </button>
      {open && <div className="flex flex-col pb-3.5 ps-8">{children}</div>}
    </div>
  );
}

/** One part of what an opened row holds: its dot, name, line and size. */
function DetailRow({ part, line }: { part: Part; line: string }) {
  return (
    <div data-part-row={part.key} className="border-divider flex items-center gap-3 border-t py-2.5">
      <Dot color={part.color} />
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        <span className="text-foreground text-xs font-medium">{part.label}</span>
        <span className="text-foreground/65 text-xs">{line}</span>
      </div>
      <span
        className={cn(
          "w-18 shrink-0 text-right text-xs",
          part.bytes > 0 ? "text-foreground" : "text-muted-foreground",
        )}
      >
        {size(part.bytes)}
      </span>
    </div>
  );
}

function Size({ bytes }: { bytes: number }) {
  return <span className="text-foreground w-18 shrink-0 text-right text-xs font-medium">{size(bytes)}</span>;
}

/** The projects not used for three months: what their Brains and conversations take, and
 * freeing that up (after a confirmation) with the projects, their repositories and files kept. */
function UnusedRow({
  projects,
  bytes,
  state,
  onState,
  onFreeUp,
}: {
  projects: ProjectUsage[];
  bytes: number;
  state: UnusedFreeUp;
  onState: (state: UnusedFreeUp) => void;
  onFreeUp: () => void;
}) {
  const n = projects.length;
  const title =
    state.phase === "done"
      ? state.title
      : n === 0
        ? "Projects not used in 3 months"
        : `${count(n, "project", "projects")} not used in 3 months`;
  const line =
    state.phase === "confirm"
      ? `Delete ${n === 1 ? "its" : "their"} ${count(
          projects.reduce((total, project) => total + project.conversations, 0),
          "conversation",
          "conversations",
        )} and move ${n === 1 ? "its Brain" : "their Brains"} to the Trash? The ${n === 1 ? "project" : "projects"}, your repositories and files stay.`
      : state.phase === "done" || state.phase === "failed"
        ? state.line
        : `${n === 1 ? "Its" : "Their"} brains and conversations take ${size(bytes)}. Your repositories and files stay.`;
  return (
    <div data-unused className="border-divider flex items-center gap-4 border-b py-3.5">
      <RoundBadge>
        <History />
      </RoundBadge>
      <div className="flex min-w-0 flex-1 flex-col gap-1">
        <span className="text-foreground text-label font-medium">{title}</span>
        <span className={cn("text-xs", state.phase === "failed" ? "text-destructive" : "text-foreground/65")}>
          {line}
        </span>
      </div>
      {state.phase === "confirm" ? (
        <>
          <Button variant="ghost" size="sm" onClick={() => onState({ phase: "idle" })}>Cancel</Button>
          <Button size="sm" onClick={onFreeUp}>Free up {size(bytes)}</Button>
        </>
      ) : state.phase === "done" || n === 0 ? null : state.phase === "freeing" ? (
        <Button variant="soft" size="sm" disabled>
          <Spinner className="animate-spin motion-reduce:animate-none" />
          Freeing up…
        </Button>
      ) : (
        <Button variant="soft" size="sm" onClick={() => onState({ phase: "confirm" })}>
          <Sparkles aria-hidden />
          Free up {size(bytes)}
        </Button>
      )}
    </div>
  );
}

/** The sort: a pill with its value and a chevron. */
function SortSelect({ value, onChange }: { value: ProjectSort; onChange: (value: ProjectSort) => void }) {
  const chosen = PROJECT_SORTS.find((option) => option.value === value);
  return (
    <DropdownMenu modal={false}>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          aria-label="Sort projects"
          className={cn(selectTrigger, "flex shrink-0 gap-1")}
        >
          {chosen?.label}
          <ChevronDown aria-hidden className="text-muted-foreground size-icon-sm shrink-0" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <DropdownMenuRadioGroup value={value} onValueChange={(next) => onChange(next as ProjectSort)}>
          {PROJECT_SORTS.map((option) => (
            <DropdownMenuRadioItem key={option.value} value={option.value} indicator={<Check className="size-icon-md" />}>
              {option.label}
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

const sumBytes = (parts: readonly Part[]) => parts.reduce((total, part) => total + part.bytes, 0);

/** Parts of several rows added up, part by part. */
function sumParts(rows: Part[][]): Part[] {
  const first = rows[0];
  if (!first) return [];
  return first.map((part, index) => ({
    ...part,
    bytes: rows.reduce((total, row) => total + (row[index]?.bytes ?? 0), 0),
  }));
}
