import { memo } from "react";

import { errorText } from "@/app/shell/dialogs/fields";
import { NavRow } from "@/components/controls";
import { ComposeEditSquare, DotsHorizontal, Download, Folder, FolderOpen, SettingsCog, Trash } from "@/components/icons";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger, Tooltip, TooltipContent, TooltipTrigger } from "@/components/overlays";
import { ConversationRow, EmptyState, NavFold, NavList, RowAction, RowActionsSlot, type RowActions } from "@/components/shared";
import { openFolder } from "@/ipc/client";
import type { Conversation, Project } from "@/ipc/generated";
import { select, setProjectExpanded } from "@/state/actions";
import { exportProjectConventions } from "@/state/brain";
import { useApp } from "@/state/store";

/**
 * A project in the sidebar: its folder and name, folding its sessions away; on hover its menu
 * (New session, Project settings…, Reveal, Export conventions, Remove…) and New session.
 */
export const ProjectRow = memo(function ProjectRow({
  project,
  sessions,
  activeId,
  drafting,
  actions,
  onSettings,
  onRemove,
}: {
  project: Project;
  sessions: Conversation[];
  activeId: string | null;
  drafting: boolean;
  actions: RowActions;
  onSettings: (project: Project) => void;
  onRemove: (project: Project) => void;
}) {
  const expanded = useApp((s) => s.expandedProjects[project.id] ?? true);
  const mac = useApp((s) => s.info?.platform === "macos");
  const repo = project.repos[0]?.path;
  const newSession = () => {
    setProjectExpanded(project.id, true);
    select({ type: "draft", kind: "session", projectId: project.id });
  };
  return (
    <li className="flex flex-col">
      <div className="group/row relative">
        <NavRow
          aria-expanded={expanded}
          data-active={drafting}
          className="pe-16"
          onClick={() => setProjectExpanded(project.id, !expanded)}
        >
          {expanded ? <FolderOpen /> : <Folder />}
          <span className="mask-fade-end min-w-0 flex-1 overflow-hidden whitespace-nowrap">
            {project.name}
          </span>
        </NavRow>
        <RowActionsSlot>
          <DropdownMenu modal={false}>
            <Tooltip>
              <TooltipTrigger asChild>
                <DropdownMenuTrigger asChild>
                  <RowAction aria-label={`Actions for ${project.name}`}>
                    <DotsHorizontal />
                  </RowAction>
                </DropdownMenuTrigger>
              </TooltipTrigger>
              <TooltipContent side="bottom">Project actions</TooltipContent>
            </Tooltip>
            <DropdownMenuContent side="bottom" align="start">
              <DropdownMenuItem onSelect={newSession}>
                <ComposeEditSquare />
                New session
              </DropdownMenuItem>
              <DropdownMenuItem onSelect={() => onSettings(project)}>
                <SettingsCog />
                Project settings…
              </DropdownMenuItem>
              {repo && (
                <DropdownMenuItem
                  onSelect={() =>
                    void openFolder(repo).catch((error: unknown) => actions.onError(errorText(error)))
                  }
                >
                  <FolderOpen />
                  {mac ? "Reveal in Finder" : "Open in File Manager"}
                </DropdownMenuItem>
              )}
              {repo && (
                <DropdownMenuItem onSelect={() => void exportProjectConventions(project.id, repo)}>
                  <Download />
                  Export conventions to AGENTS.md…
                </DropdownMenuItem>
              )}
              <DropdownMenuSeparator />
              <DropdownMenuItem variant="destructive" onSelect={() => onRemove(project)}>
                <Trash />
                Remove project…
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
          <Tooltip>
            <TooltipTrigger asChild>
              <RowAction
                aria-label={`New session in ${project.name}`}
                onClick={newSession}
              >
                <ComposeEditSquare />
              </RowAction>
            </TooltipTrigger>
            <TooltipContent side="bottom">New session</TooltipContent>
          </Tooltip>
        </RowActionsSlot>
      </div>
      <NavFold open={expanded}>
        <NavList className="pt-px pb-2">
          {sessions.map((conversation) => (
            <ConversationRow
              key={conversation.id}
              conversation={conversation}
              active={activeId === conversation.id}
              actions={actions}
              nested
            />
          ))}
          {sessions.length === 0 && <EmptyState as="li" className="ps-8" title="No sessions yet" />}
        </NavList>
      </NavFold>
    </li>
  );
});
