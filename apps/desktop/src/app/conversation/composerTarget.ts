import { createContext } from "react";

import type { BlobAttachmentAdapter } from "@/app/conversation/attachments";
import type { ResolvedDraft } from "@/app/conversation/draftSetup";
import type { MentionMemory, MentionTarget } from "@/app/conversation/Mentions";
import type { Conversation, Mention } from "@/ipc/generated";

/**
 * Queued messages pulled into the composer to edit ("Edit message"), by conversation: the slot
 * each goes back to when sent, its mentions, and the worker it was for, if one. Taken by the
 * next send in that conversation; kept while the user is elsewhere, as the draft is.
 */
export class PulledSlot {
  private slots = new Map<string, { index: number; mentions: Mention[]; worker: string | null }>();

  set(conversationId: string, index: number, mentions: Mention[], worker: string | null = null): void {
    this.slots.set(conversationId, { index, mentions, worker });
  }

  /** The worker the pulled message was for, if one. */
  worker(conversationId: string | null): string | null {
    return (conversationId !== null && this.slots.get(conversationId)?.worker) || null;
  }

  /** Whether a queued message of `conversationId` is pulled out to edit. */
  holds(conversationId: string | null): boolean {
    return conversationId !== null && this.slots.has(conversationId);
  }

  take(conversationId: string | null): { index: number; mentions: Mention[]; worker: string | null } | null {
    if (conversationId === null) return null;
    const slot = this.slots.get(conversationId) ?? null;
    this.slots.delete(conversationId);
    return slot;
  }
}

/** The app's pulled messages: a conversation's view comes and goes, its edit stays. */
export const pulledSlots = new PulledSlot();

/**
 * The composer's editable element (the Lexical input's contenteditable, not its scrolling
 * wrapper), for focusing and blurring it.
 */
export const COMPOSER_EDITABLE = "[data-slot=composer-input] .aui-lexical-input";

/** What the queue card needs from the view: the pulled slot and the attachment store. */
export type QueueControls = { pulled: PulledSlot; attachments: BlobAttachmentAdapter };

/** What the composer is attached to: a draft being set up, or a started conversation. */
export type ComposerTarget = {
  conversation: Conversation | null;
  resolved: ResolvedDraft;
  targets: readonly MentionTarget[];
  /** The session's workers its next message can go to directly (the composer's To:). */
  direct: readonly MentionTarget[];
  /** The files and conversations the `@` menu put in the text. */
  mentions: MentionMemory;
  running: boolean;
  /** Set while the latest request is stopped: continues it (the ▶ send button). */
  onResume: (() => void) | null;
  /** For pulling a queued message into the composer to edit. */
  queue: QueueControls;
};

export const ComposerTargetContext = createContext<ComposerTarget | null>(null);
