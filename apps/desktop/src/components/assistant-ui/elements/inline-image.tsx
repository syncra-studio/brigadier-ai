import { useAuiState } from "@assistant-ui/react";
import { FileImage } from "@openai/apps-sdk-ui/components/Icon";
import { type FC, type MouseEventHandler, useContext } from "react";

import {
  type AttachmentReader,
  AttachmentReaderContext,
  type AttachmentSource,
  ImagePreview,
  inlineNumberOf,
  TileRemove,
  useAttachmentUrl,
} from "@/components/assistant-ui/elements/attachment-tile";
import type { AttachmentRef } from "@/ipc/generated";
import { cn } from "@/lib/utils";

/**
 * The `[Image #n]` number of a composer attachment pasted into the text, or null for one in
 * the attachments row.
 */
export function composerInlineNumber(
  attachment: { id: string; file?: File | undefined },
  reader: AttachmentReader | null,
): number | null {
  return attachment.file
    ? inlineNumberOf(attachment.file)
    : (reader?.composerRef(attachment.id)?.inline ?? null);
}

/** What a pasted image is called where it sits in the text. */
export function inlineImageLabel(n: number): string {
  return `Image #${n}`;
}

/**
 * An image pasted into the text, where it was pasted: a chip with its thumbnail and
 * "Image #n" that opens the image preview, and in the composer a × that removes it. Without
 * its bytes, only the label.
 */
export function InlineImage({
  source,
  n,
  busy = false,
  onRemove,
}: {
  source: AttachmentSource | null;
  n: number;
  /** Still being stored: dimmed. */
  busy?: boolean;
  /** Removes the chip and its image (in the composer; a sent image stays). */
  onRemove?: MouseEventHandler<HTMLButtonElement> | undefined;
}) {
  const url = useAttachmentUrl(source ?? {});
  const label = inlineImageLabel(n);
  return (
    <span
      data-slot="inline-image"
      className={cn(
        "bg-muted text-foreground border-border inline-flex items-center gap-0.5 rounded-md border p-0.5 align-middle text-xs",
        url && "hover:bg-accent",
      )}
    >
      <ImagePreview url={url} name={label}>
        <button
          type="button"
          aria-label={`Preview ${label}`}
          title={label}
          disabled={!url}
          className="focus-visible:ring-ring inline-flex items-center gap-1 rounded-sm pe-0.5 outline-none focus-visible:ring-2"
        >
          {url ? (
            <img
              src={url}
              alt=""
              draggable={false}
              className={cn("size-icon-sm rounded-xs object-cover", busy && "opacity-50")}
            />
          ) : (
            <FileImage aria-hidden className="size-icon-xs" />
          )}
          {label}
        </button>
      </ImagePreview>
      {onRemove && (
        // Smaller than a tile's ×, its reach (the ::after) a little larger than it looks.
        <TileRemove
          label={`Remove ${label}`}
          onClick={onRemove}
          className="relative inset-auto size-icon-xs shrink-0 after:absolute after:-inset-1 [&>svg]:size-2"
        />
      )}
    </span>
  );
}

/** Image `n` in the composer's text, its bytes from the composer's attachment for it. */
export const ComposerInlineImage: FC<{
  n: number;
  onRemove: MouseEventHandler<HTMLButtonElement>;
}> = ({ n, onRemove }) => {
  const reader = useContext(AttachmentReaderContext);
  const attachment = useAuiState((s) =>
    s.composer.attachments.find((entry) => composerInlineNumber(entry, reader) === n),
  );
  const source = attachment
    ? { file: attachment.file, ref: attachment.file ? undefined : reader?.composerRef(attachment.id) }
    : null;
  return (
    <InlineImage
      source={source}
      n={n}
      busy={attachment?.status.type === "running"}
      onRemove={onRemove}
    />
  );
};

/** A sent message's image `n`, where it was pasted. */
export function MessageInlineImage({ image, n }: { image: AttachmentRef | undefined; n: number }) {
  return <InlineImage source={image ? { ref: image } : null} n={n} />;
}
