import { useAui } from "@assistant-ui/react";
import { useContext, useMemo } from "react";

import { COMPOSER_EDITABLE, ComposerTargetContext } from "@/app/conversation/composerTarget";
import { draftEnvironment, useDraft } from "@/app/conversation/draftSetup";
import { ProjectFiles } from "@/app/conversation/FilesTab";
import { SearchPanel } from "@/app/conversation/SearchPanel";
import { SourcePanel } from "@/app/conversation/SourcePanel";
import type { WorkspacePaneTab } from "@/state/workspacePane";
import { useApp } from "@/state/store";

export type ProjectDraftPaneProps = {
  projectId: string;
  tab: WorkspacePaneTab;
  searchRequest: number;
  onSearchHandled: () => void;
};

/**
 * Files, Search and Source before a session starts: its project folder. A draft has no tabs, so
 * a file goes into the message as `@path`; a session that will make its own copy only reads
 * Source and Search there.
 */
export function ProjectDraftPane({ projectId, tab, searchRequest, onSearchHandled }: ProjectDraftPaneProps) {
  const aui = useAui();
  const target = useContext(ComposerTargetContext);
  const draft = useDraft(projectId);
  const project = useApp((s) => s.projects[projectId] ?? null);
  const readOnly = draftEnvironment(draft, project) === "newWorktree";
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
