import { inlineKind, normalizedImages } from "@/lib/inlineImages";
import { useAui, useAuiState } from "@/components/conversation/kit";
import { useCallback, useEffect, useLayoutEffect, useRef } from "react";

import type { BlobAttachmentAdapter } from "@/app/conversation/attachments";
import type { MentionMemory } from "@/app/conversation/Mentions";
import { markersIn } from "@/app/conversation/inlineImages";
import type { AttachmentRef, Mention } from "@/ipc/generated";
import { type DraftMention, loadDraft, saveDraft } from "@/state/drafts";

/** How long the composer rests before its draft is kept. */
const DRAFT_SAVE_MS = 400;
/** Prompts ↑ walks back through, kept per thread. */
const HISTORY = 20;

/** The composers open now by draft scope, each taking edits made from outside it (`typeInto`). */
const typists = new Map<string, (edit: (text: string) => string) => void>();

/**
 * Edits the text of the composer open for `scope` from outside it, as where the user types
 * while a dialog has the keyboard; returns whether one was open.
 */
export function typeInto(scope: string, edit: (text: string) => string): boolean {
  const type = typists.get(scope);
  type?.(edit);
  return type !== undefined;
}

/**
 * Puts a message the daemon didn't take (a refusal, a failed send) back into the composer of
 * `scope`, before anything typed since; into its kept draft when no composer for it is open.
 */
export function putBack(scope: string, text: string): void {
  if (text === "" || typeInto(scope, (now) => (now === "" ? text : `${text}\n${now}`))) return;
  const saved = loadDraft(scope);
  const shift = saved?.text ? text.length + 1 : 0;
  saveDraft(
    scope,
    saved?.text ? `${text}\n${saved.text}` : text,
    saved?.attachments ?? [],
    (saved?.mentions ?? []).map((mention) => ({ ...mention, at: mention.at + shift })),
  );
}

/** A whole message the daemon didn't take: its text, stored attachments and mentions. */
export type SentDraft = { text: string; attachments: AttachmentRef[]; mentions: Mention[] };

/** The composers open now by draft scope, each taking a whole message back (`restoreDraft`). */
const restorers = new Map<string, (draft: SentDraft) => void>();

/**
 * Puts a whole message the daemon didn't take back into the composer of `scope`, before
 * anything typed since, with its attachments and mentions; into its kept draft when no
 * composer for it is open.
 */
export function restoreDraft(scope: string, draft: SentDraft): void {
  const restore = restorers.get(scope);
  if (restore) {
    restore(draft);
    return;
  }
  const saved = loadDraft(scope);
  const shift = saved?.text ? draft.text.length + 1 : 0;
  saveDraft(
    scope,
    saved?.text ? `${draft.text}\n${saved.text}` : draft.text,
    [...draft.attachments, ...(saved?.attachments ?? [])],
    [
      ...mentionsAt(draft.text, named(draft.mentions)),
      ...(saved?.mentions ?? []).map((mention) => ({ ...mention, at: mention.at + shift })),
    ],
  );
}

/** Each file, folder, conversation or tab mention with the `@name` it leaves in the text. */
function named(mentions: readonly Mention[]): [string, Mention][] {
  return mentions.flatMap((mention): [string, Mention][] => {
    if (mention.type === "file" || mention.type === "folder") return [[mention.path, mention]];
    if (mention.type === "chat") return [[mention.title, mention]];
    if (mention.type === "browserTab") return [[mention.title.trim() || mention.url, mention]];
    return [];
  });
}

/** Where each remembered file or conversation's `@name` stands in `text`. */
function mentionsAt(text: string, entries: Iterable<[string, Mention]>): DraftMention[] {
  const found: DraftMention[] = [];
  for (const [name, mention] of entries) {
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
 * same file or conversation. Edits for `scope` from outside it (`typeInto`) go into its text.
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

  // Before paint, so the composer appears with its draft rather than empty, then taller.
  useLayoutEffect(() => {
    if (restored.current === scope) return;
    restored.current = scope;
    const saved = loadDraft(scope);
    const draft = saved ? { ...saved, ...normalizedImages(saved.text, saved.attachments) } : null;
    const composer = aui.composer();
    if (!draft || !composer.getState().isEmpty) return;
    memory.recall(
      (draft.mentions ?? [])
        .filter(({ at, name }) => saved!.text.startsWith(`@${name}`, at))
        .map(({ mention }) => mention),
    );
    composer.setText(draft.text);
    attachments.retain(draft.attachments);
    for (const ref of draft.attachments) {
      if (ref.inline === null || markersIn(inlineKind(ref), draft.text).has(ref.inline)) {
        void composer.addAttachment(attachments.adopt(ref));
      }
    }
  }, [aui, attachments, memory, scope]);

  const keep = useCallback(() => {
    const state = aui.composer().getState();
    saveDraft(scope, state.text, attachments.draftRefs(state.text, state.attachments), mentionsAt(state.text, memory.entries()));
  }, [aui, attachments, memory, scope]);

  // Once typing rests; an emptied composer (sent or cleared) lets the draft go at once.
  useEffect(() => {
    const timer = setTimeout(keep, text === "" && held === "" ? 0 : DRAFT_SAVE_MS);
    return () => clearTimeout(timer);
  }, [keep, text, held]);

  // Leaving the conversation keeps what was typed last.
  useEffect(() => () => keep(), [keep]);

  useEffect(() => {
    const type = (edit: (text: string) => string) => {
      const composer = aui.composer();
      composer.setText(edit(composer.getState().text));
    };
    const restore = (draft: SentDraft) => {
      const composer = aui.composer();
      const now = composer.getState().text;
      memory.recall(draft.mentions);
      composer.setText(now === "" ? draft.text : `${draft.text}\n${now}`);
      for (const ref of draft.attachments) void composer.addAttachment(attachments.adopt(ref));
    };
    typists.set(scope, type);
    restorers.set(scope, restore);
    return () => {
      if (typists.get(scope) === type) typists.delete(scope);
      if (restorers.get(scope) === restore) restorers.delete(scope);
    };
  }, [aui, attachments, memory, scope]);
}

/**
 * ↑ in an empty composer walks back through the conversation's last prompts and ↓ forward
 * again; editing a recalled prompt stops the walk. A recalled prompt's mentions are
 * remembered again, so they chip and send as before, and the images and texts pasted into it
 * come back with it (those of the prompt shown before go). Returns whether the key was taken.
 */
export function usePromptHistory(
  memory: MentionMemory | null,
  attachments: BlobAttachmentAdapter | null,
  /** The composer's deleted pasted images and texts, which a recalled prompt's replace. */
  deleted: readonly Map<number, unknown>[],
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
          mentions: (message.metadata.custom["mentions"] ?? []) as Mention[],
          images: ((message.metadata.custom["attachments"] ?? []) as AttachmentRef[]).filter(
            (ref) => ref.inline !== null,
          ),
        }))
        .filter((prompt) => prompt.text.trim() !== "")
        .slice(-HISTORY);
      const back = (walk.current?.back ?? -1) + (key === "ArrowUp" ? 1 : -1);
      if (back >= prompts.length) return prompts.length > 0;
      const prompt = back < 0 ? null : prompts[prompts.length - 1 - back];
      const normalized = normalizedImages(prompt?.text ?? "", prompt?.images ?? []);
      const shown = normalized.text;
      walk.current = back < 0 ? null : { back, shown };
      if (prompt) memory?.recall(prompt.mentions);
      // Stored images come and go at once, so the chips never lack theirs or get another's.
      for (const attachment of state.attachments) {
        if (attachments?.refOf(attachment.id)?.inline != null) {
          void composer.attachment({ id: attachment.id }).remove();
        }
      }
      for (const map of deleted) map.clear();
      composer.setText(shown);
      for (const ref of normalized.attachments) {
        if (attachments && ref.inline !== null && markersIn(inlineKind(ref), shown).has(ref.inline)) {
          void composer.addAttachment(attachments.adopt(ref));
        }
      }
      return true;
    },
    [aui, memory, attachments, deleted],
  );
}
