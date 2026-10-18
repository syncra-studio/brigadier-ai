import { useRef, useState } from "react";

import { send } from "@/state/actions";
import { useBoard } from "@/state/board";
import { useApp } from "@/state/store";

/**
 * A fixed request a button sends as the user's (Revert, the pull request's buttons): one click
 * sends it once, and from then it reads as asked while the thread has it, sent, pending or queued.
 */
export function useAskRequest(conversationId: string, text: string | null) {
  const [sending, setSending] = useState(false);
  // A second click before the first one's render sends nothing.
  const clicked = useRef(false);
  const asked = useApp(
    (state) =>
      !!text &&
      (state.threads[conversationId]?.items.some((item) => item.role === "user" && item.text === text) ||
        state.pending.some((entry) => entry.conversationId === conversationId && entry.text === text)),
  );
  const queued = useBoard((state) => !!text && state.board?.conversationId === conversationId && state.board.queue.items.some((item) => item.text === text));
  const ask = () => {
    if (!text || clicked.current) return;
    clicked.current = true;
    setSending(true);
    send({ text, attachments: [], mentions: [] }, undefined, { to: conversationId }).catch(() => {
      clicked.current = false;
      setSending(false);
    });
  };
  return { asked: asked || queued || sending, ask };
}
