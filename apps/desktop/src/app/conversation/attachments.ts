import type {
  Attachment,
  AttachmentAdapter,
  CompleteAttachment,
  CreateAttachment,
  PendingAttachment,
} from "@assistant-ui/react";

import { InlineImages, inlineRefs } from "@/lib/inlineImages";
import { isPastedFile } from "@/components/assistant-ui/elements/attachment-tile";
import type { AttachmentRef } from "@/ipc/generated";
import { formatBytes } from "@/lib/format";
import { addAttachment } from "@/state/actions";

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
  readonly inline = new InlineImages();

  async uploadInline(key: string, file: File): Promise<void> {
    try {
      if (file.size > MAX_BYTES) throw new Error(`File is too large to upload (maximum ${formatBytes(MAX_BYTES)})`);
      this.inline.complete(key, await addAttachment(file));
    } catch (error) {
      this.inline.fail(key, error);
    }
  }

  messageRefs(text: string, attachments: readonly { id: string }[]): AttachmentRef[] {
    return [...this.refsOf(attachments).map((ref) => ({ ...ref, inline: false })), ...inlineRefs(text, this.inline.refs.values())];
  }

  draftRefs(attachments: readonly { id: string }[]): AttachmentRef[] {
    return [...this.refsOf(attachments), ...this.inline.refs.values()];
  }
  private readonly refs = new Map<string, AttachmentRef>();

  async *add({ file }: { file: File }): AsyncGenerator<PendingAttachment, void> {
    const base = {
      id: crypto.randomUUID(),
      type: kindOf(file.type),
      name: file.name,
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
      this.refs.set(base.id, await addAttachment(file, isPastedFile(file)));
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
