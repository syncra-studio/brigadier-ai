import { useAui } from "@/components/conversation";
import { useContext, useMemo } from "react";

import { COMPOSER_EDITABLE, ComposerTargetContext } from "@/app/conversation/composerTarget";
import { ProjectFiles } from "@/app/side-panel/FilesTab";
import { SearchPanel } from "@/app/side-panel/SearchPanel";
import { SourceHeader } from "@/app/side-panel/SourceHeader";
import { SourcePanel } from "@/app/side-panel/SourcePanel";
import { useApp } from "@/state/store";
import { useUserWorktrees } from "@/state/workspaceCache";
import type { WorkspacePaneTab } from "@/state/workspacePane";

export type ProjectDraftPaneProps = {
  projectId: string;
  tab: WorkspacePaneTab;
  searchRequest: number;
  onSearchHandled: () => void;
};

/**
 * Files, Search and Source before a session starts: its project folder. A draft has no tabs, so
 * a file goes into the message as `@path`. Stage, discard, commit, Search and Replace work on the
 * folder as it is, whichever way the session will start; Source lists the worktrees the user made
 * beside it.
 */
export function ProjectDraftPane({ projectId, tab, searchRequest, onSearchHandled }: ProjectDraftPaneProps) {
  const aui = useAui();
  const target = useContext(ComposerTargetContext);
  const searchTarget = useMemo(() => ({ type: "project", projectId }) as const, [projectId]);
  const mention = (path: string) => {
    // Remembered as the `@` menu does, so it shows as a file chip and is sent as one.
    target?.mentions.record({ type: "file", path }, path);
    const composer = aui.composer();
    const text = composer.getState().text;
    composer.setText(`${text}${text && !text.endsWith(" ") ? " " : ""}@${path} `);
    requestAnimationFrame(() => document.querySelector<HTMLElement>(COMPOSER_EDITABLE)?.focus());
  };
  return tab === "files" ? (
    <ProjectFiles projectId={projectId} searchRequest={searchRequest} onSearchHandled={onSearchHandled} onOpen={mention} />
  ) : tab === "search" ? (
    <SearchPanel target={searchTarget} onOpen={mention} />
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
