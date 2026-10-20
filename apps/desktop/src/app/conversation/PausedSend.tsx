import { useEffect, useRef } from "react";
import { create } from "zustand";

import { Button } from "@/components/controls";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/overlays";
import { useBoard } from "@/state/board";

/**
 * A send held while the queue is paused: `send(true)` puts it in the queue, `send(false)` sends
 * it now. `field` had the keyboard when it asked, and has it back once the dialog closes.
 */
type Ask = { conversationId: string; waiting: number; send: (queue: boolean) => void; field: Element | null };

const usePausedSendAsk = create<{ ask: Ask | null }>(() => ({ ask: null }));

/** Conversations whose next send goes into the queue (Keep paused), taken by the send. */
const queued = new Set<string>();

/** Whether the send being made was asked to wait in the paused queue. */
export function takeQueueSend(conversationId: string | null): boolean {
  return conversationId !== null && queued.delete(conversationId);
}

/**
 * Sending while the conversation's queue is paused with messages in it asks first: `send` is
 * called once the user picks, with whether the message joins the paused queue. Returns whether
 * it asked (else the caller sends as usual).
 */
export function askPausedSend(conversationId: string | null, send: () => void): boolean {
  if (conversationId === null) return false;
  const board = useBoard.getState().board;
  if (board?.conversationId !== conversationId || !board.queue.paused || board.queue.items.length === 0) return false;
  usePausedSendAsk.setState({
    ask: {
      conversationId,
      waiting: board.queue.items.length,
      field: document.activeElement,
      send: (queue) => {
        if (queue) queued.add(conversationId);
        send();
      },
    },
  });
  return true;
}

function close() {
  usePausedSendAsk.setState({ ask: null });
}

/**
 * "Send message?": the queue is paused (after an interrupt), so the message either goes now,
 * ahead of the queued ones, or waits in the queue with them. Closing it sends nothing; the
 * message stays in the composer.
 */
export function PausedSendDialog({ conversationId }: { conversationId: string }) {
  const ask = usePausedSendAsk((s) => (s.ask?.conversationId === conversationId ? s.ask : null));
  const sendNow = useRef<HTMLButtonElement>(null);
  // The field asked from, kept while the dialog animates out.
  const field = useRef<Element | null>(null);
  useEffect(() => {
    if (!ask) return;
    field.current = ask.field;
    requestAnimationFrame(() => sendNow.current?.focus());
  }, [ask]);
  const pick = (queue: boolean) => {
    close();
    ask?.send(queue);
  };
  return (
    <Dialog open={ask !== null} onOpenChange={(open) => !open && close()}>
      <DialogContent
        className="max-w-md"
        onCloseAutoFocus={(event) => {
          if (!(field.current instanceof HTMLElement) || !field.current.isConnected) return;
          event.preventDefault();
          field.current.focus();
        }}
      >
        <DialogHeader>
          <DialogTitle>Send message?</DialogTitle>
          <DialogDescription>
            The queue is paused.{" "}
            {ask?.waiting === 1 ? "One message waits" : `${ask?.waiting ?? 0} messages wait`} until
            you resume it. Send this one now, or keep it in the queue with them.
          </DialogDescription>
        </DialogHeader>
        <DialogFooter>
          <Button type="button" variant="ghost" onClick={() => pick(true)}>
            Keep paused
          </Button>
          <Button ref={sendNow} type="button" onClick={() => pick(false)}>
            Send now
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
