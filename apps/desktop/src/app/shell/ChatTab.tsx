import { ChatActions } from "@/app/shell/ChatActions";
import { noAutoscroll, TAB } from "@/app/shell/SessionTab";
import { TabMenu } from "@/app/shell/TabMenu";
import { Archive, Chat, X } from "@/components/icons";
import { SidebarActivityMark, SidebarError, statusWords } from "@/components/shared";
import type { Conversation } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { closeSession } from "@/state/actions";
import { CHAT_TAB, selectTab } from "@/state/sessionTabs";
import type { SessionStatus } from "@/state/tabStates";

/**
 * The session bar's first tab: the chat under its title, with its state marks and its ⋯; never
 * moved. Its × closes the session's view, which goes on working.
 */
export function ChatTab({
  conversation,
  title,
  active,
  separator,
  status,
  error,
  onRename,
}: {
  conversation: Conversation;
  title: string;
  active: boolean;
  separator: true | undefined;
  status: SessionStatus | null;
  error: boolean;
  onRename: () => void;
}) {
  const id = conversation.id;
  const archived = conversation.lifecycle === "archived";
  return (
    <TabMenu conversationId={id} tab={null} onRename={archived ? undefined : onRename}>
      <div
        data-tab-id={CHAT_TAB}
        data-separator={separator}
        data-active={active || undefined}
        className={cn(
          TAB,
          active
            ? "bg-panel-tab shadow-session-tab text-foreground"
            : "text-toolbar-foreground hover:bg-toolbar-hover",
        )}
      >
        <button
          type="button"
          role="tab"
          aria-selected={active}
          tabIndex={active ? 0 : -1}
          title={archived ? `${title} (Archived)` : status ? `${title} · ${statusWords(status)}` : title}
          aria-label={archived ? `${title} (Archived)` : undefined}
          onMouseDown={noAutoscroll}
          onMouseUp={(event) => { if (event.button === 1) { event.preventDefault(); closeSession(id); } }}
          onClick={() => selectTab(id, CHAT_TAB)}
          onDoubleClick={archived ? undefined : onRename}
          className="flex h-full min-w-0 flex-1 items-center gap-1 rounded-sm"
        >
          {/* The sidebar row's own marks, without its state words: its error glyph leads, its spinner trails. */}
          {archived ? <Archive aria-hidden className="size-icon-sm shrink-0" /> : error ? <SidebarError conversationId={id} /> : <Chat aria-hidden className="size-icon-sm shrink-0" />}
          <span className="session-tab-title min-w-0 flex-1 overflow-hidden text-start whitespace-nowrap">{title}</span>
          {/* The session in front is open: it is never unread. */}
          {!archived && <SidebarActivityMark conversationId={id} unread={false} />}
        </button>
        <ChatActions
          conversation={conversation}
          onRename={onRename}
          compact
          className="shrink-0"
        />
        {/* Closes the view only: the session goes on working and stays in the sidebar. */}
        <button
          type="button"
          aria-label={`Close ${title} tab`}
          onClick={(event) => { event.stopPropagation(); closeSession(id); }}
          className={cn(
            "hover:bg-toolbar-hover flex size-5 shrink-0 items-center justify-center rounded-full opacity-60 hover:opacity-100",
            !active && "invisible group-focus-within/tab:visible group-hover/tab:visible",
          )}
        >
          <X aria-hidden className="size-icon-xs" />
        </button>
      </div>
    </TabMenu>
  );
}
