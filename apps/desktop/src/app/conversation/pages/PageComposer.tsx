import { ArrowUp } from "@openai/apps-sdk-ui/components/Icon";
import { useState } from "react";

import { shownBranch } from "@/app/conversation/blocks";
import { enterAction } from "@/app/conversation/composerKeys";
import { TooltipIconButton } from "@/components/assistant-ui/tooltip-icon-button";
import type { Mention } from "@/ipc/generated";
import { send } from "@/state/actions";
import { useBoard, workingRequest } from "@/state/board";
import type { PageEditing } from "@/state/pageEditing";
import { CHAT_TAB, selectTab } from "@/state/sessionTabs";
import { useApp } from "@/state/store";

function pageOf(mentions: readonly Mention[] | undefined): string | null {
  const page = mentions?.find((mention) => mention.type === "page");
  return page?.type === "page" ? page.id : null;
}

/**
 * Asks the session's agent to work on this page: the message goes to the session's chat with
 * the page attached, so the agent may read and change this page (and no other) until the user
 * writes again. The page is saved first; a page that can't be saved isn't sent.
 */
export function PageComposer({ conversationId, pageId, title, editing }: {
  conversationId: string; pageId: string; title: string; editing: PageEditing;
}) {
  const [text, setText] = useState("");
  const [sending, setSending] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const archived = useApp((s) => s.conversations[conversationId]?.lifecycle === "archived");
  const mac = useApp((s) => s.info?.platform === "macos");
  const sendShortcut = useApp((s) => s.settings.sendShortcut);
  const followUp = useApp((s) => s.settings.followUp);
  const head = useBoard((s) => (s.board?.conversationId === conversationId ? s.board.head : null));
  // The agent works on this page while its turn runs for the latest message, sent from here.
  const latestPage = useApp((s) => {
    const pending = s.pending.filter((entry) => entry.conversationId === conversationId).at(-1);
    if (pending) return pageOf(pending.mentions);
    const thread = s.threads[conversationId];
    const branch = thread ? shownBranch(thread.items, head, thread.hasMore) : [];
    return pageOf(branch.findLast((item) => item.role === "user")?.mentions);
  });
  const running = useBoard((s) => {
    const board = s.board;
    return board?.conversationId === conversationId
      && (board.run === "running" || board.run === "starting" || workingRequest(board) !== null);
  });
  const working = running && latestPage === pageId;

  /** `other`: sent the follow-up's other way (steered in instead of queued, or the reverse). */
  const submit = async (other = false) => {
    const message = text.trim();
    if (!message || sending || archived) return;
    setSending(true);
    setProblem(null);
    try {
      const unsaved = await editing.flush(pageId);
      if (unsaved) { setProblem(`Not sent. ${unsaved}`); return; }
      // Words typed while the page saved stay for the next message.
      setText((now) => (now.trim() === message ? "" : now));
      const steer = running && (followUp === "steer") !== other;
      await send(
        { text: message, attachments: [], mentions: [{ type: "page", id: pageId, title }] },
        undefined,
        { to: conversationId, lane: steer ? "steer" : "auto" },
      ).catch((error: unknown) => {
        setText((now) => now || message);
        setProblem(`Not sent: ${error instanceof Error ? error.message : String(error)}`);
      });
    } finally {
      setSending(false);
    }
  };

  return (
    <div className="page-composer-dock">
      <div className="page-composer-column">
        {working && (
          <p role="status" className="page-composer-status">
            <span>Working on this page</span>
            <span aria-hidden>·</span>
            <button type="button" className="page-composer-link" onClick={() => selectTab(conversationId, CHAT_TAB)}>See in chat</button>
          </p>
        )}
        {problem && <p role="alert" className="page-composer-problem">{problem}</p>}
        <form
          className="bg-composer rounded-composer shadow-hairline flex items-end gap-2 p-2 backdrop-blur-lg"
          onSubmit={(event) => { event.preventDefault(); void submit(); }}
        >
          <textarea
            aria-label="Work with this page"
            placeholder={archived ? "This session is archived" : "Work with this page…"}
            disabled={archived}
            rows={1}
            value={text}
            onChange={(event) => { setText(event.target.value); setProblem(null); }}
            onKeyDown={(event) => {
              if (event.key !== "Enter" || event.nativeEvent.isComposing) return;
              const action = enterAction(event, mac, sendShortcut);
              if (!action) return;
              event.preventDefault();
              void submit(action === "other");
            }}
            className="page-composer-input"
          />
          <TooltipIconButton
            tooltip="Send"
            type="submit"
            variant="default"
            size="icon-md"
            className="rounded-capsule shrink-0"
            disabled={archived || sending || !text.trim()}
          >
            <ArrowUp />
          </TooltipIconButton>
        </form>
      </div>
    </div>
  );
}
