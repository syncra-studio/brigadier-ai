import { useAui, useAuiState } from "@assistant-ui/react";
import { useCallback, useEffect, useRef } from "react";

import type { BlobAttachmentAdapter } from "@/app/conversation/attachments";
import type { InlineImages } from "@/lib/inlineImages";
import type { MentionMemory } from "@/app/conversation/Mentions";
import type { AttachmentRef, Mention } from "@/ipc/generated";
import { type DraftMention, loadDraft, saveDraft } from "@/state/drafts";

/** How long the composer rests before its draft is kept. */
const DRAFT_SAVE_MS = 400;
/** Prompts ↑ walks back through, kept per thread. */
const HISTORY = 20;

/** Where each remembered file or conversation's `@name` stands in `text`. */
function mentionsAt(text: string, memory: MentionMemory): DraftMention[] {
  const found: DraftMention[] = [];
  for (const [name, mention] of memory.entries()) {
    for (let at = text.indexOf(`@${name}`); at >= 0; at = text.indexOf(`@${name}`, at + 1)) {
      found.push({ at, name, mention });
    }
  }
  return found.toSorted((a, b) => a.at - b.at);
}

/**
 * Keeps the composer's draft for `scope` (see state/drafts): restores it when the composer
 * opens empty, and saves text, stored attachments and mentions once typing rests and when it
 * closes. A restored mention whose `@name` is still where it was chips again, and sends the
 * same file or conversation.
 */
export function useComposerDraft(
  scope: string,
  attachments: BlobAttachmentAdapter,
  memory: MentionMemory,
): void {
  const aui = useAui();
  const text = useAuiState((s) => s.composer.text);
  // Changes when an attachment is added, stored or removed.
  const held = useAuiState((s) =>
    s.composer.attachments.map((attachment) => `${attachment.id}:${attachment.status.type}`).join(),
  );
  const restored = useRef<string | null>(null);

  useEffect(() => {
    if (restored.current === scope) return;
    restored.current = scope;
    const draft = loadDraft(scope);
    const composer = aui.composer();
    if (!draft || !composer.getState().isEmpty) return;
    memory.recall(
      (draft.mentions ?? [])
        .filter(({ at, name }) => draft.text.startsWith(`@${name}`, at))
        .map(({ mention }) => mention),
    );
    attachments.inline.recall(draft.text, draft.attachments);
    composer.setText(draft.text);
    for (const ref of draft.attachments.filter((attachment) => !attachment.inline)) void composer.addAttachment(attachments.adopt(ref));
  }, [aui, attachments, memory, scope]);

  const keep = useCallback(() => {
    const state = aui.composer().getState();
    saveDraft(scope, state.text, attachments.draftRefs(state.text, state.attachments), mentionsAt(state.text, memory));
  }, [aui, attachments, memory, scope]);

  useEffect(() => attachments.inline.subscribe(keep), [attachments, keep]);

  // Once typing rests; an emptied composer (sent or cleared) lets the draft go at once.
  useEffect(() => {
    const timer = setTimeout(keep, text === "" && held === "" ? 0 : DRAFT_SAVE_MS);
    return () => clearTimeout(timer);
  }, [keep, text, held]);

  // Leaving the conversation keeps what was typed last.
  useEffect(() => () => keep(), [keep]);
}

/**
 * ↑ in an empty composer walks back through the conversation's last prompts and ↓ forward
 * again; editing a recalled prompt stops the walk. A recalled prompt's mentions are
 * remembered again, so they chip and send as before. Returns whether the key was taken.
 */
export function usePromptHistory(
  memory: MentionMemory | null,
  images?: InlineImages,
): (key: "ArrowUp" | "ArrowDown") => boolean {
  const aui = useAui();
  // How far back the composer shows, and the text it put there.
  const walk = useRef<{ back: number; shown: string } | null>(null);
  return useCallback(
    (key: "ArrowUp" | "ArrowDown") => {
      const composer = aui.composer();
      const state = composer.getState();
      if (walk.current && state.text !== walk.current.shown) walk.current = null;
      if (!walk.current && !(key === "ArrowUp" && state.isEmpty)) return false;
      const prompts = aui
        .thread()
        .getState()
        .messages.filter((message) => message.role === "user")
        .map((message) => ({
          text: message.content
            .flatMap((part) => (part.type === "text" ? [part.text] : []))
            .join("\n"),
          attachments: (message.metadata.custom["attachments"] ?? []) as AttachmentRef[],
          mentions: (message.metadata.custom["mentions"] ?? []) as Mention[],
        }))
        .filter((prompt) => prompt.text.trim() !== "")
        .slice(-HISTORY);
      const back = (walk.current?.back ?? -1) + (key === "ArrowUp" ? 1 : -1);
      if (back >= prompts.length) return prompts.length > 0;
      const prompt = back < 0 ? null : prompts[prompts.length - 1 - back];
      const shown = prompt?.text ?? "";
      walk.current = back < 0 ? null : { back, shown };
      if (prompt) {
        memory?.recall(prompt.mentions);
        images?.recall(prompt.text, prompt.attachments);
      }
      composer.setText(shown);
      return true;
    },
    [aui, memory, images],
  );
}
