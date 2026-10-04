import type { Unstable_DirectiveFormatter } from "@assistant-ui/react";
import type { DirectiveChipProps } from "@assistant-ui/react-lexical";
import { useLexicalComposerContext } from "@lexical/react/LexicalComposerContext";
import { $getNearestNodeFromDOMNode } from "lexical";
import { useEffect, useSyncExternalStore } from "react";

import { InlineImage } from "@/app/conversation/InlineImage";
import { registerInlineImages } from "@/app/conversation/inlineImageEditor";
import { INLINE_IMAGE, type InlineImages } from "@/lib/inlineImages";

export function ComposerImage({ chip, images }: { chip: DirectiveChipProps; images: InlineImages }) {
  const [editor] = useLexicalComposerContext();
  useSyncExternalStore(images.subscribe, images.snapshot);
  const upload = images.uploads.get(chip.directiveId);
  const ref = chip.directiveType === INLINE_IMAGE
    ? images.refs.get(chip.directiveId)
    : upload?.status === "ready" ? upload.ref : undefined;
  const error = upload?.status === "failed" ? upload.error : null;
  return (
    <span
      className="bg-muted mx-1 inline-flex items-center rounded-md align-middle"
      contentEditable={false}
    >
      {ref ? <InlineImage attachment={ref} /> : (
        <span
          role={error ? "alert" : "status"}
          title={error ?? chip.label}
          className={error ? "text-destructive px-2 text-xs" : "text-muted-foreground px-2 text-xs"}
        >
          {error ?? "Uploading image…"}
        </span>
      )}
      <button
        type="button"
        aria-label={`Remove ${chip.label}`}
        className="px-1 text-xs"
        onClick={(event) => {
          const element = event.currentTarget;
          editor.update(() => $getNearestNodeFromDOMNode(element)?.remove());
        }}
      >
        ×
      </button>
    </span>
  );
}

/** Resolves old pending nodes too, so undo across completion never resurrects an upload. */
export function InlineImagePlugin({ images, formatter }: {
  images: InlineImages;
  formatter: Unstable_DirectiveFormatter;
}) {
  const [editor] = useLexicalComposerContext();
  useEffect(() => {
    const dispose = registerInlineImages(editor, images, formatter);
    return () => {
      dispose();
      images.close();
    };
  }, [editor, images, formatter]);
  return null;
}
