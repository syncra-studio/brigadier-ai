import { type ComponentProps, useContext } from "react";

import {
  attachmentOpens,
  openInDefaultApp,
  refuse,
} from "@/components/conversation/elements/attachment-open";
import {
  FileTile,
  ImageTile,
  kindOf,
  PasteShortened,
  pasteTooLong,
} from "@/components/conversation/elements/attachment-tile";
import type { ViewerImage } from "@/components/conversation/elements/image-viewer";
import { type OpenPastedText, PastedTextOpenerContext } from "@/components/conversation/elements/pasted-text";
import type { AttachmentRef } from "@/ipc/generated";
import { formatBytes } from "@/lib/format";
import { cn } from "@/lib/utils";

/** What a click on a file's card does; with no reader to open it in (a fixture board), nothing. */
function cardAction(file: AttachmentRef, openPasted: OpenPastedText | null): (() => void) | undefined {
  switch (attachmentOpens(file)) {
    case "viewer":
      return undefined;
    case "reader":
      return openPasted ? () => openPasted(file.inline ?? null, file) : undefined;
    case "app":
      return () => openInDefaultApp(file);
    case "refused":
      return () => refuse(file);
  }
}

/**
 * The Message attachment element (assistant-ui): a sent message's files show as
 * images as thumbnails that open a preview, other files as cards with their kind and size. A
 * whole card opens its file as a Sources row does: pasted text, text files and scripts read only
 * (a session's tab, elsewhere over the thread), other files in the default app.
 */
export function MessageAttachments({
  attachments,
  className,
  ...props
}: Omit<ComponentProps<"ul">, "children"> & { attachments: readonly AttachmentRef[] }) {
  const openPasted = useContext(PastedTextOpenerContext);
  attachments = attachments.filter((ref) => ref.inline == null);
  if (attachments.length === 0) return null;
  // The message's images, stepped between in the viewer.
  const images = attachments.filter((ref) => ref.mime.startsWith("image/"));
  const gallery = images.map((ref): ViewerImage => ({ name: ref.name, source: { ref } }));
  return (
    <ul
      data-slot="message-attachments"
      aria-label="Attachments"
      className={cn("flex flex-wrap items-end gap-2", className)}
      {...props}
    >
      {attachments.map((attachment) => (
        <li key={attachment.id} className="flex">
          {attachment.mime.startsWith("image/") ? (
            <ImageTile
              source={{ ref: attachment }}
              name={attachment.name}
              sent
              gallery={images.length > 1 ? { images: gallery, index: images.indexOf(attachment) } : undefined}
            />
          ) : (
            <FileTile
              name={attachment.name}
              mime={attachment.mime}
              title={attachment.pasted ? "Pasted text" : undefined}
              onOpen={cardAction(attachment, openPasted)}
              openLabel={`Open ${attachment.pasted ? "Pasted text" : attachment.name}`}
              detail={
                attachment.pasted && pasteTooLong(attachment.bytes) ? (
                  <PasteShortened />
                ) : (
                  `${kindOf(attachment.name, attachment.mime)} · ${formatBytes(attachment.bytes)}`
                )
              }
            />
          )}
        </li>
      ))}
    </ul>
  );
}

/** "2 files · 1.2 MB": a one-line summary, for queued messages. */
export function attachmentSummary(attachments: readonly AttachmentRef[]): string | null {
  if (attachments.length === 0) return null;
  const bytes = attachments.reduce((sum, attachment) => sum + attachment.bytes, 0);
  const noun = attachments.length === 1 ? attachments[0]?.name ?? "1 file" : `${attachments.length} files`;
  return `${noun} · ${formatBytes(bytes)}`;
}

