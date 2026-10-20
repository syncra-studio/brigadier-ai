import type {
  Attachment,
  AttachmentAdapter,
  CompleteAttachment,
  CreateAttachment,
  PendingAttachment,
} from "@assistant-ui/react";
import { useSyncExternalStore } from "react";

import { draftRefs, inlineKey, inlineRefs } from "@/lib/inlineImages";
import { inlineNumberOf, isPastedFile } from "@/components/conversation/elements/attachment-tile";
import type { AttachmentRef } from "@/ipc/generated";
import { fileName, formatBytes } from "@/lib/format";
import { addAttachment } from "@/state/actions";

/** Send's tip while attached files are still being stored. */
export const FILES_UPLOADING = "Files uploading…";

/** Whether one of `attachments` is still being stored: the message waits for it. */
export function uploading(attachments: readonly Attachment[]): boolean {
  return attachments.some((attachment) => attachment.status.type === "running");
}

/** Dropped files whose bytes are still being read (macOS), by composer: its Send waits (#330). */
const reading = new Map<string, number>();
const readingHeard = new Set<() => void>();

function subscribeReading(heard: () => void): () => void {
  readingHeard.add(heard);
  return () => readingHeard.delete(heard);
}

/** Holds `owner`'s Send while `read` (a dropped file's read and its attaching) is under way. */
export function whileReading(owner: string, read: Promise<unknown>): Promise<unknown> {
  const change = (by: number) => {
    const left = (reading.get(owner) ?? 0) + by;
    if (left > 0) reading.set(owner, left);
    else reading.delete(owner);
    for (const heard of readingHeard) heard();
  };
  change(1);
  return read.finally(() => change(-1));
}

/** Whether a dropped file is still being read for `owner`'s message: it waits for it. */
export function readingFiles(owner: string): boolean {
  return reading.has(owner);
}

/** {@link readingFiles}, as it changes. */
export function useReadingFiles(owner: string): boolean {
  return useSyncExternalStore(subscribeReading, () => reading.has(owner));
}

/** The daemon's limit per attachment (MAX_ATTACHMENT_BYTES in the core). */
const MAX_BYTES = 10 * 1024 * 1024;

function kindOf(mime: string): "image" | "document" | "file" {
  if (mime.startsWith("image/")) return "image";
  if (mime.startsWith("text/") || mime === "application/pdf") return "document";
  return "file";
}

/**
 * Attachments for the composer: each file is stored in the daemon's blob store as soon as it
 * is added, so sending only passes references. Attachment-only messages are allowed.
 */
export class BlobAttachmentAdapter implements AttachmentAdapter {
  accept = "*";
  /** The images and pasted texts the text has held, by kind and number (`image:1`, `paste:1`). */
  private readonly retained = new Map<string, AttachmentRef>();

  retain(refs: Iterable<AttachmentRef>): void {
    for (const ref of refs) {
      const key = inlineKey(ref);
      if (key) this.retained.set(key, ref);
    }
  }

  messageRefs(text: string, attachments: readonly { id: string }[]): AttachmentRef[] {
    const refs = this.refsOf(attachments);
    return [...refs.filter((ref) => ref.inline === null), ...inlineRefs(text, refs)];
  }

  draftRefs(text: string, attachments: readonly { id: string }[]): AttachmentRef[] {
    const refs = this.refsOf(attachments);
    return draftRefs(text, refs.filter((ref) => ref.inline === null), this.retained.values());
  }
  private readonly refs = new Map<string, AttachmentRef>();

  async *add({ file }: { file: File }): AsyncGenerator<PendingAttachment, void> {
    const base = {
      id: crypto.randomUUID(),
      type: kindOf(file.type),
      name: fileName(file.name),
      contentType: file.type || "application/octet-stream",
      file,
    };
    if (file.size > MAX_BYTES) {
      yield {
        ...base,
        status: {
          type: "incomplete",
          reason: "error",
          message: `File is too large to upload (maximum ${formatBytes(MAX_BYTES)})`,
        },
      };
      return;
    }
    yield { ...base, status: { type: "running", reason: "uploading", progress: 0 } };
    try {
      const ref = await addAttachment(file, isPastedFile(file), base.name);
      const stored = { ...ref, inline: inlineNumberOf(file) };
      this.refs.set(base.id, stored);
      this.retain([stored]);
      yield { ...base, status: { type: "requires-action", reason: "composer-send" } };
    } catch (error) {
      yield {
        ...base,
        status: {
          type: "incomplete",
          reason: "error",
          message: error instanceof Error ? error.message : String(error),
        },
      };
    }
  }

  async remove(attachment: Attachment): Promise<void> {
    // The blob stays until the daemon's garbage collection; nothing references it.
    this.refs.delete(attachment.id);
  }

  async send(attachment: PendingAttachment): Promise<CompleteAttachment> {
    if (!this.refs.has(attachment.id)) {
      throw new Error(`${attachment.name} was not stored; remove it and attach it again.`);
    }
    return { ...attachment, status: { type: "complete" }, content: [] };
  }

  /**
   * An attachment already in the blob store (a queued message pulled back into the composer),
   * ready for `composer.addAttachment`: sending passes the same reference again.
   */
  adopt(ref: AttachmentRef): CreateAttachment {
    const id = crypto.randomUUID();
    this.refs.set(id, ref);
    this.retain([ref]);
    return { id, type: kindOf(ref.mime), name: ref.name, contentType: ref.mime, content: [] };
  }

  /** The stored reference behind a composer attachment, once stored. */
  refOf(id: string): AttachmentRef | undefined {
    return this.refs.get(id);
  }

  /** The stored references for a sent message's attachments, in order. */
  refsOf(attachments: readonly { id: string }[]): AttachmentRef[] {
    return attachments.flatMap((attachment) => {
      const ref = this.refs.get(attachment.id);
      return ref ? [ref] : [];
    });
  }
}
