import { DetailRow } from "@/app/settings/storage/DetailRow";
import { ExpandableRow } from "@/app/settings/storage/ExpandableRow";
import {
  canGoBytes,
  conversationsLine,
  count,
  isUnused,
  projectParts,
  size,
  usedWords,
  type Part,
} from "@/app/settings/storage/model";
import { Folder, FolderOpen, Sparkles } from "@/components/icons";
import { Button } from "@/components/controls";
import { revealPath } from "@/ipc/client";
import type { ProjectUsage, StorageReport } from "@/ipc/generated";
import { shortPath } from "@/lib/format";
import { cn } from "@/lib/utils";

/** A project's row and, open, what's in it with its own cleanup. */
export function ProjectRows({
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
