import {
  AttachmentPrimitive,
  ComposerPrimitive,
  useAui,
  useAuiState,
} from "@assistant-ui/react";
import { ChevronRight, Warning } from "@openai/apps-sdk-ui/components/Icon";
import { type FC, type ReactNode, useContext } from "react";

import {
  AttachmentReaderContext,
  type AttachmentSource,
  FileTile,
  ImageTile,
  isPastedFile,
  kindOf,
  PasteShortened,
  pasteTooLong,
  readAttachmentText,
  TileRemove,
  useAttachmentText,
} from "@/components/assistant-ui/elements/attachment-tile";
import { composerInlineNumber } from "@/components/assistant-ui/elements/inline-image";
import { Spinner } from "@/components/glyphs/spinner";

/**
 * One composer attachment (the assistant-ui Attachment element): an
 * image is a thumbnail that opens a preview, a long paste a "Pasted text" card that can go
 * back into the text field, any other file a card with its kind. Each has a remove ×. An
 * image pasted into the text isn't one: it shows in the text, unless it couldn't be stored.
 */
const ComposerAttachment: FC = () => {
  const reader = useContext(AttachmentReaderContext);
  const id = useAuiState((s) => s.attachment.id);
  const name = useAuiState((s) => s.attachment.name);
  const mime = useAuiState((s) => s.attachment.contentType ?? "");
  const file = useAuiState((s) => ("file" in s.attachment ? s.attachment.file : undefined));
  const failure = useAuiState((s) =>
    s.attachment.status.type === "incomplete" ? s.attachment.status.message : undefined,
  );
  const state = useAuiState((s) => {
    const status = s.attachment.status;
    if (status.type === "running") return "uploading";
    if (status.type === "incomplete" && status.reason === "error") return "error";
    return "ready";
  });
  // An image pasted into the text shows there, as its chip; here only if it failed, to say why.
  if (state !== "error" && composerInlineNumber({ id, file }, reader) !== null) return null;
  const source: AttachmentSource = { file, ref: file ? undefined : reader?.composerRef(id) };
  const pasted = file ? isPastedFile(file) : source.ref?.pasted === true;
  const remove = (
    <AttachmentPrimitive.Remove asChild>
      <TileRemove
        label={pasted ? "Remove pasted text" : `Remove ${name}`}
        onHover={mime.startsWith("image/")}
      />
    </AttachmentPrimitive.Remove>
  );
  const spinner = <Spinner aria-label="Uploading" className="size-icon-md animate-spin" />;

  let tile;
  if (state === "error") {
    tile = (
      <FileTile
        name={name}
        mime={mime}
        detail={failure ?? "Couldn't attach this file"}
        icon={<Warning aria-label="Upload failed" className="text-destructive size-icon-md" />}
        className="border-destructive/50"
      >
        {remove}
      </FileTile>
    );
  } else if (mime.startsWith("image/")) {
    tile = (
      <ImageTile source={source} name={name} busy={state === "uploading"}>
        {state === "uploading" && (
          <span className="pointer-events-none absolute inset-0 flex items-center justify-center">
            {spinner}
          </span>
        )}
        {remove}
      </ImageTile>
    );
  } else if (pasted) {
    tile = <PastedText source={source} busy={state === "uploading"} remove={remove} />;
  } else {
    tile = (
      <FileTile
        name={name}
        mime={mime}
        detail={state === "uploading" ? "Attaching…" : kindOf(name, mime)}
        icon={state === "uploading" ? spinner : undefined}
      >
        {remove}
      </FileTile>
    );
  }
  return (
    <AttachmentPrimitive.Root
      data-slot="composer-attachment"
      data-state={state}
      className="animate-in fade-in-0 zoom-in-95 duration-200 motion-reduce:animate-none"
    >
      {tile}
    </AttachmentPrimitive.Root>
  );
};

/**
 * The "Pasted text" card: the paste's first line and "Show in text field ›", which
 * puts the text back into the composer and drops the attachment. A paste too long to go to
 * the model whole says so.
 */
const PastedText: FC<{ source: AttachmentSource; busy: boolean; remove: ReactNode }> = ({
  source,
  busy,
  remove,
}) => {
  const aui = useAui();
  const reader = useContext(AttachmentReaderContext);
  const text = useAttachmentText(source);
  const firstLine = text?.split("\n").find((line) => line.trim() !== "")?.trim();
  const tooLong = pasteTooLong(source.file?.size ?? source.ref?.bytes ?? 0);
  const show = async () => {
    const pasted = await readAttachmentText(source, reader);
    const composer = aui.composer();
    const current = composer.getState().text;
    composer.setText(current === "" || current.endsWith("\n") ? current + pasted : `${current}\n${pasted}`);
    aui.attachment().remove();
  };
  return (
    <FileTile
      name="Pasted text"
      mime="text/plain"
      title={firstLine ?? "Pasted text"}
      detail={
        tooLong ? (
          <PasteShortened />
        ) : (
          <button
            type="button"
            disabled={busy}
            onClick={() => void show()}
            className="hover:text-foreground focus-visible:ring-ring inline-flex items-center gap-0.5 underline underline-offset-2 outline-none focus-visible:ring-2"
          >
            Show in text field
            <ChevronRight aria-hidden className="size-icon-xs" />
          </button>
        )
      }
    >
      {remove}
    </FileTile>
  );
};

export const ComposerAttachments: FC = () => (
  <div
    data-slot="composer-attachments"
    className="flex w-full flex-wrap items-end gap-2 px-1 pt-1 pb-1.5 empty:hidden"
  >
    <ComposerPrimitive.Attachments>{() => <ComposerAttachment />}</ComposerPrimitive.Attachments>
  </div>
);
