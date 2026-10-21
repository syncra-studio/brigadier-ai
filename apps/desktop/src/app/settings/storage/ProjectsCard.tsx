import { useState } from "react";

import { DetailRow } from "@/app/settings/storage/DetailRow";
import { ExpandableRow, shares, sumBytes } from "@/app/settings/storage/ExpandableRow";
import { ProjectRows } from "@/app/settings/storage/ProjectRows";
import { StorageSize } from "@/app/settings/storage/StorageSize";
import { type UnusedFreeUp, UnusedRow } from "@/app/settings/storage/UnusedRow";
import {
  PROJECTS_PAGE,
  PROJECT_SORTS,
  TOP_PROJECTS,
  count,
  isUnused,
  listProjects,
  projectParts,
  sharedParts,
  unusedProjects,
  type Part,
  type ProjectSort,
} from "@/app/settings/storage/model";
import { ChevronDown, ChevronRight, ChevronUp, Folders, Search, Stack } from "@/components/icons";
import { Button, SettingsSelect } from "@/components/controls";
import { SettingsCard, UsageBar } from "@/components/shared";
import type { StorageReport } from "@/ipc/generated";

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
      <SettingsCard inset split={false} data-projects="all">
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
          <SettingsSelect label="Sort projects" value={sort} options={PROJECT_SORTS} onChange={setSort} />
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
      </SettingsCard>
    );
  }

  const ordered = listProjects(report.projects, "largest");
  const top = ordered.slice(0, TOP_PROJECTS);
  const rest = ordered.slice(TOP_PROJECTS);
  const restParts = sumParts(rest.map(projectParts));
  const restUnused = rest.filter((project) => isUnused(project, nowMs)).length;
  const shared = sharedParts(report);
  return (
    <SettingsCard inset data-projects="top">
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
          <UsageBar parts={shares(restParts, total)} size="md" className="w-50" />
          <StorageSize bytes={sumBytes(restParts)} />
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
    </SettingsCard>
  );
}

/** Parts of several rows added up, part by part. */
function sumParts(rows: Part[][]): Part[] {
  const first = rows[0];
  if (!first) return [];
  return first.map((part, index) => ({
    ...part,
    bytes: rows.reduce((total, row) => total + (row[index]?.bytes ?? 0), 0),
  }));
}
