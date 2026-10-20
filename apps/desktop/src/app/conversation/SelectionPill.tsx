import { useAui, useAuiState } from "@/components/conversation";
import { type FC, useContext, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";

import { COMPOSER_EDITABLE } from "@/app/conversation/composerTarget";
import { ViewContext } from "@/app/conversation/viewContext";
import { ChatCompose, Plus } from "@/components/icons";
import { request } from "@/ipc/client";
import { saveDraft } from "@/state/drafts";
import { openSideChatTab } from "@/state/sessionTabs";
import { useApp } from "@/state/store";
import { toast } from "@/state/toasts";
import { showHighlight } from "@/lib/highlightRules";

/** The highlight that keeps what was added to the chat marked in the answer. */
const ANNOTATION = "thread-annotation" as const;
/** The pill keeps this far inside the window's edges, and this far above the selection. */
const EDGE_PX = 8;
const ABOVE_PX = 8;

/** Text as a Markdown quote, one `> ` per line. */
export function quoted(text: string): string {
  return text.trim().split("\n").map((line) => `> ${line}`.trimEnd()).join("\n");
}

/** The highlight registry, where the webview has one. */
function annotations(): Highlight | null {
  if (typeof CSS === "undefined" || !("highlights" in CSS) || typeof Highlight === "undefined") return null;
  let highlight = CSS.highlights.get(ANNOTATION);
  if (!highlight) {
    highlight = new Highlight();
    CSS.highlights.set(ANNOTATION, highlight);
  }
  return highlight;
}

/**
 * Opens a new side chat of `conversationId` with `text` in its composer, quoted, for the user to
 * ask about it, and shows its tab.
 */
export async function askInSideChat(conversationId: string, text: string): Promise<void> {
  const { conversation } = await request({ method: "openSideChat", conversationId, sideChatId: crypto.randomUUID() });
  useApp.setState((state) => ({ conversations: { ...state.conversations, [conversation.id]: conversation } }));
  saveDraft(conversation.id, `${quoted(text)}\n\n`, [], []);
  openSideChatTab(conversationId, conversation.id);
}

type Shown = { range: Range; text: string; x: number; y: number };

/**
 * Over text selected in one of the thread's answers, a pill: "Add to chat" quotes it into the
 * composer and keeps it marked in the answer; "Ask in side chat" (in a session) opens a side
 * chat to ask about it. It follows the selection, hides while the pointer is down, on a scroll
 * and once the selection goes, and never shows for the composer's own text.
 */
export const SelectionPill: FC = () => {
  const aui = useAui();
  const view = useContext(ViewContext);
  const anchor = useRef<HTMLSpanElement>(null);
  const pill = useRef<HTMLDivElement>(null);
  const [shown, setShown] = useState<Shown | null>(null);
  const [width, setWidth] = useState(0);
  const empty = useAuiState((s) => s.composer.text.trim() === "");

  useEffect(() => {
    let frame = 0;
    let down = false;
    const read = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => {
        const selection = document.getSelection();
        const thread = anchor.current?.closest(".aui-thread-root");
        if (down || !thread || !selection || selection.isCollapsed || selection.rangeCount === 0) return setShown(null);
        const range = selection.getRangeAt(0);
        const start = range.startContainer.parentElement;
        const end = range.endContainer.parentElement;
        // Within this thread's answers only: never the composer, a bubble of the user's, or another view.
        const inAnswer = (node: Element | null) => !!node?.closest("[data-role=assistant]") && node.closest(".aui-thread-root") === thread
          && !node.closest("[data-thread-scroll-footer]");
        const text = selection.toString();
        if (!inAnswer(start) || !inAnswer(end) || !text.trim()) return setShown(null);
        const box = range.getBoundingClientRect();
        setShown({ range: range.cloneRange(), text, x: box.left + box.width / 2, y: box.top - ABOVE_PX });
      });
    };
    const press = (event: PointerEvent) => {
      if (pill.current?.contains(event.target as Node)) return;
      down = true;
      setShown(null);
    };
    const release = () => {
      down = false;
      read();
    };
    // A scroll outside the pill moves the text away from it.
    const scrolled = () => setShown(null);
    document.addEventListener("selectionchange", read);
    document.addEventListener("pointerdown", press, true);
    document.addEventListener("pointerup", release, true);
    document.addEventListener("keyup", read, true);
    document.addEventListener("scroll", scrolled, true);
    return () => {
      cancelAnimationFrame(frame);
      document.removeEventListener("selectionchange", read);
      document.removeEventListener("pointerdown", press, true);
      document.removeEventListener("pointerup", release, true);
      document.removeEventListener("keyup", read, true);
      document.removeEventListener("scroll", scrolled, true);
    };
  }, []);

  // Sent (the composer emptied): what it quoted is no longer marked.
  useEffect(() => {
    if (!empty) return;
    annotations()?.clear();
    showHighlight(ANNOTATION, false);
  }, [empty]);

  useEffect(() => {
    if (shown && pill.current) setWidth(pill.current.offsetWidth);
  }, [shown]);

  const conversation = view.conversation;
  const sideChats = !view.embedded && conversation?.kind === "session" && !conversation.sideOf;

  const add = () => {
    if (!shown) return;
    const composer = aui.composer();
    const current = composer.getState().text;
    const quote = quoted(shown.text);
    composer.setText(current.trim() ? `${current.trimEnd()}\n\n${quote}\n\n` : `${quote}\n\n`);
    annotations()?.add(shown.range);
    showHighlight(ANNOTATION, true);
    document.getSelection()?.removeAllRanges();
    setShown(null);
    // This pill's own composer: a side-by-side chat has one of its own.
    requestAnimationFrame(() =>
      anchor.current?.closest(".aui-composer-root")?.querySelector<HTMLElement>(COMPOSER_EDITABLE)?.focus(),
    );
  };
  const ask = () => {
    if (!shown || !conversation) return;
    const { text } = shown;
    document.getSelection()?.removeAllRanges();
    setShown(null);
    askInSideChat(conversation.id, text).catch((error: unknown) =>
      toast(`Couldn’t open a side chat: ${error instanceof Error ? error.message : String(error)}`, { tone: "error" }),
    );
  };

  const half = width / 2;
  const left = shown ? Math.min(Math.max(shown.x, EDGE_PX + half), window.innerWidth - EDGE_PX - half) : 0;
  return (
    <>
      <span ref={anchor} hidden />
      {shown &&
        createPortal(
          <div
            ref={pill}
            role="toolbar"
            tabIndex={-1}
            aria-label="Selected text"
            data-slot="selection-pill"
            // Pressing a button keeps the selection it acts on.
            onMouseDown={(event) => event.preventDefault()}
            style={{ left, top: Math.max(EDGE_PX, shown.y) }}
            className="bg-popover text-popover-foreground border-border rounded-capsule fixed z-50 flex -translate-x-1/2 -translate-y-full items-center gap-0.5 border p-0.5 text-sm shadow-lg"
          >
            <button type="button" onClick={add} className="hover:bg-foreground/8 rounded-capsule flex items-center gap-1.5 px-2.5 py-1">
              <Plus aria-hidden className="size-icon-sm" />
              Add to chat
            </button>
            {sideChats && (
              <button type="button" onClick={ask} className="hover:bg-foreground/8 rounded-capsule flex items-center gap-1.5 px-2.5 py-1">
                <ChatCompose aria-hidden className="size-icon-sm" />
                Ask in side chat
              </button>
            )}
          </div>,
          document.body,
        )}
    </>
  );
};
