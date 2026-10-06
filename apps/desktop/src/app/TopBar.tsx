import { Folder } from "@openai/apps-sdk-ui/components/Icon";
import type { ReactNode } from "react";
import { useShallow } from "zustand/react/shallow";

import { Badge } from "@/components/ui/badge";
import type { Lifecycle } from "@/ipc/generated";
import { useApp } from "@/state/store";

/** The names the daemon gives a conversation until its first message titles it. */
const UNTITLED = new Set(["New session", "New chat"]);

/** The open thread's title, or "" while it has none of its own (drafts too). */
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
          title: conversation && !UNTITLED.has(conversation.title) ? conversation.title : "",
          lifecycle: conversation?.lifecycle ?? null,
          session: conversation?.kind === "session",
        };
      }
      if (selection.type === "draft" && selection.kind === "session") {
        return {
          project: s.projects[selection.projectId]?.name ?? null,
          title: "",
          lifecycle: null,
          session: false,
        };
      }
      return { project: null, title: "", lifecycle: null, session: false };
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
  const { project, title, lifecycle, session } = useTitle();
  const connection = useApp((s) => s.connection.status);

  return (
    // In the titlebar strip, above the page surface. It starts clear of Back, Forward and the
    // sidebar toggle, which stay where they are. Its end sits in the surface inset already, so
    // pe-1 leaves its buttons 8px from the window's edge.
    <header
      data-tauri-drag-region
      className="h-titlebar ease-sidebar ps-clear-3 flex shrink-0 items-center gap-1 pe-1 transition-[padding] duration-300 motion-reduce:transition-none"
    >
      <div data-tauri-drag-region className="flex min-w-0 flex-1 items-center gap-1.5 text-sm">
        {session && title && (
          <Folder
            aria-label={project ?? undefined}
            className="text-muted-foreground size-icon-md shrink-0"
          />
        )}
        {!title ? null : onRename ? (
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
