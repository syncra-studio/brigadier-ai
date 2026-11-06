import type { ReactNode } from "react";

import { Comment, Folder, ProviderGlyph } from "@/components/icons";
import { TooltipContent } from "@/components/overlays";
import { SidebarRowMark } from "@/components/shared/session-status";
import type { Conversation } from "@/ipc/generated";
import { useProjectIcon } from "@/lib/projectIcon";
import { projectInitials, tildePath } from "@/lib/rowLabels";
import { modelName, useModelGroups } from "@/lib/setup";
import { tokenPx } from "@/lib/tokens";
import { cn } from "@/lib/utils";
import { useRowMark } from "@/state/tabStates";

/** Where a session works: its worktree once created, else the repository. Chats work nowhere. */
export function workingDirectory(conversation: Conversation): string | null {
  const setup = conversation.setup;
  if (setup?.type !== "session") return null;
  return (setup.environment.type === "newWorktree" && setup.environment.path) || setup.repo;
}

/** A project's own icon in a square of its slot's size; drawn through `<img>`, so an SVG's scripts don't run. */
function ProjectIconImage({ icon, onBroken }: { icon: string; onBroken: () => void }) {
  return <img data-slot="project-icon" src={icon} alt="" draggable={false} className="size-icon-md shrink-0 rounded-xs object-contain" onError={onBroken} />;
}

/** A project row's mark: the project's icon when it has one that loads, else `fallback`. */
export function ProjectIcon({ projectId, fallback }: { projectId: string; fallback: ReactNode }) {
  const { icon, broken } = useProjectIcon(projectId);
  return icon === null ? fallback : <ProjectIconImage icon={icon} onBroken={broken} />;
}

/**
 * A row's badge: its project's icon when it has one that loads, else its initials in grey; a bubble
 * in grey for a chat (`name` null).
 */
export function ProjectBadge({ name, projectId }: { name: string | null; projectId: string | null }) {
  const { icon, broken } = useProjectIcon(name === null ? null : projectId);
  if (icon !== null) {
    return <span data-slot="row-badge" aria-hidden className="flex shrink-0"><ProjectIconImage icon={icon} onBroken={broken} /></span>;
  }
  return (
    <span data-slot="row-badge" aria-hidden className="bg-foreground/8 text-foreground/70 text-badge flex size-icon-md shrink-0 items-center justify-center rounded-xs font-semibold">
      {name === null ? <Comment className="size-icon-xs" /> : projectInitials(name)}
    </span>
  );
}

const CARD_LINE = "text-foreground/85 text-label flex min-h-6 min-w-0 items-center gap-2";

/** What a card says beyond the title; mounted only while the card shows. */
function RowCardBody({ conversation, project }: { conversation: Conversation; project: string | null }) {
  const { hover } = useRowMark(conversation.id);
  const groups = useModelGroups();
  const setup = conversation.setup;
  const model = setup?.type === "session" ? setup.orchestrator : setup?.type === "chat" ? setup.model : null;
  const folder = workingDirectory(conversation);
  return (
    <>
      <p data-slot="card-title" className="mb-1 line-clamp-3 font-medium wrap-anywhere">{conversation.title}</p>
      {hover && (
        <div data-slot="card-status" className={cn(CARD_LINE, "-ms-0.5")}>
          <SidebarRowMark conversationId={conversation.id} />
          <span className="-ms-0.5">{hover}</span>
        </div>
      )}
      <div className="border-divider my-1.5 border-t" />
      <div data-slot="card-project" className={CARD_LINE}>
        <ProjectBadge name={project} projectId={conversation.projectId} />
        <span className="min-w-0 wrap-anywhere">{project ?? "Chat"}</span>
      </div>
      {model && (
        <div data-slot="card-model" className={CARD_LINE}>
          <ProviderGlyph provider={model.provider} className="size-icon-md shrink-0" />
          <span className="min-w-0 wrap-anywhere">{modelName(groups, model)}</span>
        </div>
      )}
      {folder && (
        <div data-slot="card-folder" className={cn(CARD_LINE, "text-muted-foreground items-start py-0.75")}>
          <Folder aria-hidden className="size-icon-md shrink-0" />
          <span className="min-w-0 break-all">{tildePath(folder)}</span>
        </div>
      )}
    </>
  );
}

/**
 * A row's card, the content of a Tooltip around it (on hover and on keyboard focus alike): beside
 * the sidebar, its title, what its mark means, its project, model and folder.
 */
export function RowCard({ conversation, project }: { conversation: Conversation; project: string | null }) {
  return (
    <TooltipContent
      data-slot="row-card"
      side="right"
      align="start"
      // Past the row's own inset, clear of the sidebar's edge.
      sideOffset={tokenPx("--spacing") * 4}
      className="rounded-surface w-66 max-w-none flex-col items-stretch gap-0 px-3 py-2.5 text-start"
    >
      <RowCardBody conversation={conversation} project={project} />
    </TooltipContent>
  );
}
