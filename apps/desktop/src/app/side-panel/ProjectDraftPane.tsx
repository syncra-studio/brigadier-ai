import { useAui } from "@/components/conversation";
import { useContext, useMemo } from "react";

import { COMPOSER_EDITABLE, ComposerTargetContext } from "@/app/conversation/composerTarget";
import { useDraft } from "@/app/conversation/draftSetup";
import { ProjectFiles } from "@/app/side-panel/FilesTab";
import { SearchPanel } from "@/app/side-panel/SearchPanel";
import { SourcePanel } from "@/app/side-panel/SourcePanel";
import type { WorkspacePaneTab } from "@/state/workspacePane";

export type ProjectDraftPaneProps = {
  projectId: string;
  tab: WorkspacePaneTab;
  searchRequest: number;
  onSearchHandled: () => void;
};

/**
 * Files, Search and Source before a session starts: its project folder. A draft has no tabs, so
 * a file goes into the message as `@path`. A session that will work on its own worktree only
 * reads Source and Search there; one in the local checkout works on them.
 */
export function ProjectDraftPane({ projectId, tab, searchRequest, onSearchHandled }: ProjectDraftPaneProps) {
  const aui = useAui();
  const target = useContext(ComposerTargetContext);
  const readOnly = useDraft(projectId).environment !== "localCheckout";
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
    <SearchPanel target={searchTarget} onOpen={mention} readOnly={readOnly} />
  ) : (
    <SourcePanel target={{ type: "project", projectId, readOnly }} />
  );
}
