import { Folder } from "@openai/apps-sdk-ui/components/Icon";
import type { ReactNode } from "react";
import { useShallow } from "zustand/react/shallow";

import { Badge } from "@/components/ui/badge";
import { useSidebar } from "@/components/ui/sidebar";
import type { Lifecycle } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { useApp } from "@/state/store";

function useTitle(): {
  project: string | null;
  title: string;
  lifecycle: Lifecycle | null;
  session: boolean;
} {
  return useApp(
    useShallow((s) => {
      const { selection } = s;
      if (selection.type === "conversation") {
        const conversation = s.conversations[selection.id];
        const project = conversation?.projectId
          ? (s.projects[conversation.projectId]?.name ?? null)
          : null;
        return {
          project,
          title: conversation?.title ?? "",
          lifecycle: conversation?.lifecycle ?? null,
          session: conversation?.kind === "session",
        };
      }
      if (selection.type === "draft" && selection.kind === "session") {
        return {
          project: s.projects[selection.projectId]?.name ?? null,
          title: "New session",
          lifecycle: null,
          session: false,
        };
      }
      return { project: null, title: "New chat", lifecycle: null, session: false };
    }),
  );
}

/**
 * The header over the thread: a session's folder icon and the title (click to
 * rename), then the conversation's own controls (`children`) on the right.
 */
export function TopBar({
  onRename,
  children,
}: {
  onRename?: (() => void) | undefined;
  children?: ReactNode;
}) {
  const { state } = useSidebar();
  const { project, title, lifecycle, session } = useTitle();
  const connection = useApp((s) => s.connection.status);

  return (
    <header
      data-tauri-drag-region
      className={cn(
        // In the titlebar strip, above the page surface. With the sidebar panel closed it
        // starts clear of the sidebar toggle, which stays where it is. Its end sits in the
        // surface inset already, so pe-1 leaves its buttons 8px from the window's edge.
        "h-titlebar ease-sidebar flex shrink-0 items-center gap-1 ps-3 pe-1 transition-[padding] duration-300 motion-reduce:transition-none",
        state === "collapsed" && "ps-titlebar-clear",
      )}
    >
      <div data-tauri-drag-region className="flex min-w-0 flex-1 items-center gap-1.5 text-sm">
        {session && (
          <Folder
            aria-label={project ?? undefined}
            className="text-muted-foreground size-icon-md shrink-0"
          />
        )}
        {onRename ? (
          <button
            type="button"
            title={project ? `${project} · Rename` : "Rename"}
            onClick={onRename}
            className="hover:text-foreground/80 min-w-0 truncate font-medium transition-colors"
          >
            {title}
          </button>
        ) : (
          <span className="truncate font-medium">{title}</span>
        )}
        {lifecycle === "archived" && (
          <Badge variant="outline" className="ms-1" title="Archived: restore it from Settings → Archived chats to continue.">
            Archived
          </Badge>
        )}
      </div>

      {connection !== "connected" && (
        <Badge variant="warning" role="status">
          {connection === "connecting"
            ? "Connecting to core…"
            : "Reconnecting to core…"}
        </Badge>
      )}
      {children}
    </header>
  );
}
