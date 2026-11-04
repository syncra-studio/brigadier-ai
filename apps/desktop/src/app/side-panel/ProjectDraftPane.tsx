import { useCallback, useMemo } from "react";

import { ProjectFiles } from "@/app/side-panel/FilesTab";
import { SearchPanel } from "@/app/side-panel/SearchPanel";
import { SourceHeader } from "@/app/side-panel/SourceHeader";
import { SourcePanel } from "@/app/side-panel/SourcePanel";
import { openFileTab } from "@/state/sessionTabs";
import { useApp } from "@/state/store";
import { useUserWorktrees } from "@/state/workspaceCache";
import type { WorkspacePaneTab } from "@/state/workspacePane";

export type ProjectDraftPaneProps = {
  projectId: string;
  tab: WorkspacePaneTab;
  searchRequest: number;
  onSearchHandled: () => void;
  replaceTabId?: string | undefined;
  onFileOpened?: (() => void) | undefined;
};

/**
 * Files, Search and Source before a session starts: its project folder. A file or a change opens
 * in the draft's tabs, which carry over when the session starts. Stage, discard, commit, Search and Replace work on the
 * folder as it is, whichever way the session will start; Source lists the worktrees the user made
 * beside it.
 */
export function ProjectDraftPane({ projectId, tab, searchRequest, onSearchHandled, replaceTabId, onFileOpened }: ProjectDraftPaneProps) {
  const searchTarget = useMemo(() => ({ type: "project", projectId }) as const, [projectId]);
  const openResult = useCallback((path: string, line: number, keep: boolean) => openFileTab(`draft:${projectId}`, path, { line, preview: !keep }), [projectId]);
  return tab === "files" ? (
    <ProjectFiles projectId={projectId} searchRequest={searchRequest} onSearchHandled={onSearchHandled} replaceTabId={replaceTabId} onFileOpened={onFileOpened} />
  ) : tab === "search" ? (
    <SearchPanel target={searchTarget} onOpen={openResult} />
  ) : (
    <DraftSource projectId={projectId} />
  );
}

/** Source before a session: the project's folder alone, or with the user's worktrees a stacked row for each. */
function DraftSource({ projectId }: { projectId: string }) {
  const worktrees = useUserWorktrees(projectId);
  const folder = useApp((s) => s.projects[projectId]?.repos[0]?.path) ?? "";
  if (!worktrees?.length) return <SourcePanel target={{ type: "project", projectId }} />;
  return (
    <div data-slot="source-rows" className="flex min-h-0 flex-1 flex-col">
      <SourceHeader />
      <div className="min-h-0 flex-1 overflow-y-auto pb-2">
        <SourcePanel
          key={folder}
          target={{ type: "project", projectId }}
          row={{ name: baseName(folder), title: folder, rowKey: `project:${projectId}`, startOpen: true }}
        />
        {worktrees.map((path) => (
          <SourcePanel
            key={path}
            target={{ type: "worktree", projectId, path }}
            row={{ name: baseName(path), title: path, rowKey: `worktree:${projectId}:${path}`, startOpen: false }}
          />
        ))}
      </div>
    </div>
  );
}

function baseName(path: string): string {
  return path.slice(path.lastIndexOf("/") + 1);
}
