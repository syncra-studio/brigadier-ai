import {
  type Attachment,
  type CreateAttachment,
  type Unstable_DirectiveFormatter,
  type Unstable_TriggerItem,
  useAui,
  useAuiState,
} from "@assistant-ui/react";
import { $createDirectiveNodeWithFormatter, $isDirectiveNode } from "@assistant-ui/react-lexical";
import { useLexicalComposerContext } from "@lexical/react/LexicalComposerContext";
import { $dfs, mergeRegister } from "@lexical/utils";
import { Chat } from "@openai/apps-sdk-ui/components/Icon";
import {
  $getRoot,
  $getSelection,
  $isRangeSelection,
  $isTextNode,
  COMMAND_PRIORITY_NORMAL,
  COPY_COMMAND,
  CUT_COMMAND,
  type EditorState,
  HISTORY_MERGE_TAG,
  type LexicalNode,
  KEY_ARROW_DOWN_COMMAND,
  KEY_ARROW_UP_COMMAND,
  KEY_DOWN_COMMAND,
  type LexicalEditor,
  PASTE_COMMAND,
  PASTE_TAG,
  SKIP_DOM_SELECTION_TAG,
} from "lexical";
import { useCallback, useContext, useEffect, useMemo, useRef } from "react";

import { WorkerGlyph, useWorkerName } from "@/app/conversation/WorkerChip";
import type { BlobAttachmentAdapter } from "@/app/conversation/attachments";
import { usePromptHistory } from "@/app/conversation/composerDraft";
import { ComposerTargetContext } from "@/app/conversation/composerTarget";
import {
  imageMarkerAt,
  imageMarkersIn,
  nextImageNumber,
  pastedImageNumbers,
  pastesInline,
  renumberImages,
  sameImages,
  splitAtImages,
} from "@/app/conversation/inlineImages";
import { mentionOf } from "@/app/conversation/Mentions";
import { usePullQueued } from "@/app/conversation/QueueCard";
import {
  type AttachmentReader,
  AttachmentReaderContext,
  type AttachmentSource,
  inlineImageFile,
  PASTE_AS_ATTACHMENT_CHARS,
  pastedTextFile,
} from "@/components/assistant-ui/elements/attachment-tile";
import {
  ChipComposerInput,
  chipFormatter,
  imageChip,
  type MentionLook,
} from "@/components/assistant-ui/elements/composer-chips";
import { composerInlineNumber } from "@/components/assistant-ui/elements/inline-image";
import {
  cancelDictation,
  registerInserter,
  startDictation,
  stopDictation,
  useDictation,
} from "@/state/dictation";
import { NEW_CHAT_SCOPE } from "@/state/drafts";

export type ComposerInputProps = {
  placeholder: string;
  autoFocus: boolean;
};

/**
 * The text field (assistant-ui's Lexical input): grows with its text up to a quarter of the
 * window, then scrolls, the top line fading under the edge once scrolled. Mentions, inline
 * code and links show as chips while the text stays plain.
 */
export default function ComposerEditor({
  placeholder,
  autoFocus,
}: ComposerInputProps) {
  const target = useContext(ComposerTargetContext);
  const memory = target?.mentions ?? null;
  const targets = target?.targets;
  useEffect(() => memory?.setWorkers(targets ?? []), [memory, targets]);
  // One formatter per memory: a new one would rebuild every chip.
  const formatter = useMemo(
    () => chipFormatter((text, at) => memory?.match(text, at) ?? null, imageMarkerAt),
    [memory],
  );
  // Images whose chip was deleted, by number, for undo to bring back.
  const deleted = useMemo(() => new Map<number, File | CreateAttachment>(), []);
  const onMention = useCallback(
    (item: Unstable_TriggerItem) => {
      const mention = mentionOf(item);
      if (mention && mention.type !== "task") memory?.record(mention, item.label);
    },
    [memory],
  );
  return (
    <ChipComposerInput
      formatter={formatter}
      mentionLook={mentionLook}
      onMention={onMention}
      placeholder={placeholder}
      autoFocus={autoFocus}
      // Esc stops only on a second press (useEscToStop).
      cancelOnEscape={false}
      aria-label="Message input"
    >
      <ComposerKeys formatter={formatter} deleted={deleted} />
      <InlineImageSync deleted={deleted} />
    </ChipComposerInput>
  );
}

/** A worker on its mention chip by its name; its `task-N` stays in the text it sends. */
function MentionedWorker({ taskId, label }: { taskId: string; label: string }) {
  return useWorkerName(taskId) ?? label;
}

/** A worker's glyph or a conversation's icon on its mention chip; files keep their type's. */
const mentionLook: MentionLook = ({ directiveType, directiveId, label }) => {
  const id = directiveId.slice(directiveId.indexOf(":") + 1);
  if (directiveType === "task") {
    return { icon: <WorkerGlyph taskId={id} />, name: <MentionedWorker taskId={id} label={label} /> };
  }
  if (directiveType === "chat") return { icon: <Chat />, name: label };
  return null;
};

/**
 * The composer's keys and paste: ↑ in an empty field edits the last queued message, else
 * walks back through the conversation's prompts (↓ forward); a long paste becomes a
 * "Pasted text" attachment; pasted images go in the text at the caret as `[Image #n]` chips,
 * and other pasted files attach directly; chips cut or copied and pasted again bring their
 * images along, at any length; ⌃⇧D starts dictating at the caret and stops (or
 * cancels) again. The `@` and `/` menus take their keys first.
 */
function ComposerKeys({
  formatter,
  deleted,
}: {
  formatter: Unstable_DirectiveFormatter;
  deleted: DeletedImages;
}) {
  const [editor] = useLexicalComposerContext();
  const aui = useAui();
  const reader = useContext(AttachmentReaderContext);
  const pull = usePullQueued();
  const target = useContext(ComposerTargetContext);
  const history = usePromptHistory(
    target?.mentions ?? null,
    target?.queue.attachments ?? null,
    deleted,
  );
  const adapter = target?.queue.attachments ?? null;
  const owner = target?.conversation?.id ?? NEW_CHAT_SCOPE;
  const dictation = useDictation(owner);
  // Dictated text lands at the caret (or at the end, if the field never had one), spaced
  // from the word before it.
  useEffect(
    () =>
      registerInserter(
        owner,
        (text) =>
          new Promise<void>((resolve) => {
            editor.update(
              () => {
                let selection = $getSelection();
                if (!$isRangeSelection(selection)) {
                  $getRoot().selectEnd();
                  selection = $getSelection();
                }
                if (!$isRangeSelection(selection)) return;
                const node = selection.anchor.getNode();
                const before = $isTextNode(node)
                  ? node.getTextContent().slice(0, selection.anchor.offset)
                  : "";
                selection.insertText(before && !/\s$/.test(before) ? ` ${text}` : text);
              },
              // The composer has the text once the editor has updated: Lexical runs its update
              // listeners, assistant-ui's sync among them, before `onUpdate`. (Not a frame
              // later, since a window in the background may not draw one.)
              { onUpdate: () => resolve() },
            );
            editor.focus();
          }),
      ),
    [editor, owner],
  );
  const dictating = dictation.phase.type === "recording";
  // Pressed again while the model downloads or the microphone opens, it cancels (as the button does).
  const opening = dictation.phase.type === "downloading" || dictation.phase.type === "starting";
  const canDictate = dictation.available && dictation.phase.type !== "transcribing";
  useEffect(() => {
    const arrow = (key: "ArrowUp" | "ArrowDown") => (event: KeyboardEvent) => {
      if (event.isComposing) return false;
      if (key === "ArrowUp" && pull && aui.composer().getState().isEmpty) void pull(-1);
      else if (!history(key)) return false;
      event.preventDefault();
      return true;
    };
    // Lexical puts the selection's text on the clipboard; its images are noted here, and the
    // clipboard marked as theirs.
    const copy = (event: ClipboardEvent | KeyboardEvent | null) => {
      copiedImages = copiedFrom($getSelection()?.getTextContent() ?? "", aui, reader);
      if (copiedImages && event instanceof ClipboardEvent) {
        event.clipboardData?.setData(COPIED_IMAGES_TYPE, copiedImages.token);
      }
      return false;
    };
    return mergeRegister(
      editor.registerCommand(COPY_COMMAND, copy, COMMAND_PRIORITY_NORMAL),
      editor.registerCommand(CUT_COMMAND, copy, COMMAND_PRIORITY_NORMAL),
      editor.registerCommand(KEY_ARROW_UP_COMMAND, arrow("ArrowUp"), COMMAND_PRIORITY_NORMAL),
      editor.registerCommand(KEY_ARROW_DOWN_COMMAND, arrow("ArrowDown"), COMMAND_PRIORITY_NORMAL),
      editor.registerCommand(
        KEY_DOWN_COMMAND,
        (event) => {
          const dictate =
            event.ctrlKey && event.shiftKey && !event.altKey && !event.metaKey && event.code === "KeyD";
          if (!dictate || !canDictate) return false;
          event.preventDefault();
          if (dictating) void stopDictation();
          else if (opening) cancelDictation();
          else void startDictation(owner);
          return true;
        },
        COMMAND_PRIORITY_NORMAL,
      ),
      editor.registerCommand(
        PASTE_COMMAND,
        (event) => {
          if (!(event instanceof ClipboardEvent) || !event.clipboardData) return false;
          const files = [...event.clipboardData.files];
          const text = event.clipboardData.getData("text/plain");
          const composer = aui.composer();
          // Chips cut or copied with their images come back as chips, however long their text.
          const copied = copiedImagesOn(event.clipboardData, text);
          if (files.length === 0 && (copied.size > 0 || text.length < PASTE_AS_ATTACHMENT_CHARS)) {
            if (imageMarkersIn(text).size === 0) return false;
            event.preventDefault();
            pasteWithImages(text, copied, { editor, formatter, aui, reader, adapter, deleted });
            return true;
          }
          event.preventDefault();
          if (pastesInline({ files, uris: event.clipboardData.getData("text/uri-list") })) {
            pasteImages(files, { editor, formatter, aui, reader, adapter, deleted });
            return true;
          }
          const attach = files.length > 0 ? files : [pastedTextFile(text)];
          for (const file of attach) void composer.addAttachment(file);
          return true;
        },
        COMMAND_PRIORITY_NORMAL,
      ),
    );
  }, [
    editor,
    aui,
    reader,
    adapter,
    formatter,
    deleted,
    pull,
    history,
    owner,
    canDictate,
    dictating,
    opening,
  ]);
  return null;
}

/** The images of deleted chips by their number, until the composer is sent or cleared. */
type DeletedImages = Map<number, File | CreateAttachment>;

/**
 * The images of the chips last cut or copied from a composer, by number, with the text that
 * went on the clipboard and the token marking it: pasting that clipboard again, in any
 * composer, brings them along.
 */
let copiedImages: { text: string; token: string; images: Map<number, AttachmentSource> } | null =
  null;

/** Where a cut or copy marks the clipboard as holding `copiedImages`' text. */
const COPIED_IMAGES_TYPE = "application/x-brigadier-images";

/** The images of the chips in `text` (cut or copied from the composer), if any. */
function copiedFrom(
  text: string,
  aui: ReturnType<typeof useAui>,
  reader: AttachmentReader | null,
): typeof copiedImages {
  const { attachments } = aui.composer().getState();
  const images = new Map<number, AttachmentSource>();
  for (const n of imageMarkersIn(text)) {
    const attachment = attachments.find((entry) => composerInlineNumber(entry, reader) === n);
    if (attachment) images.set(n, { file: attachment.file, ref: reader?.composerRef(attachment.id) });
  }
  return images.size > 0 ? { text, token: crypto.randomUUID(), images } : null;
}

/**
 * The images that came along with pasted `text`: those of the last cut or copy, if this is
 * still its clipboard (not the same text copied since from elsewhere).
 */
function copiedImagesOn(clipboard: DataTransfer, text: string): Map<number, AttachmentSource> {
  if (copiedImages?.text !== text || clipboard.getData(COPIED_IMAGES_TYPE) !== copiedImages.token) {
    return new Map();
  }
  return copiedImages.images;
}

/** What pasting into a composer works with. */
type PasteContext = {
  editor: LexicalEditor;
  formatter: Unstable_DirectiveFormatter;
  aui: ReturnType<typeof useAui>;
  reader: AttachmentReader | null;
  adapter: BlobAttachmentAdapter | null;
  deleted: DeletedImages;
};

/** Merges of pasted images, one after another: each sees the composer as the last left it. */
let merging = Promise.resolve();

function afterMerges(merge: () => Promise<void>) {
  merging = merging.then(merge).catch((error: unknown) => console.error("merge failed", error));
}

/** Images' content hashes (the blob store, too, keys bytes by their content), memoized. */
const fileHashes = new WeakMap<File, Promise<string | null>>();
const storedHashes = new Map<string, Promise<string | null>>();

async function digest(bytes: Blob): Promise<string> {
  const hash = await crypto.subtle.digest("SHA-256", await bytes.arrayBuffer());
  return [...new Uint8Array(hash)].map((byte) => byte.toString(16).padStart(2, "0")).join("");
}

/** The content hash of an image's bytes, or null if they can't be read. */
function contentHash(
  { file, ref }: AttachmentSource,
  reader: AttachmentReader | null,
): Promise<string | null> {
  if (file) {
    let hash = fileHashes.get(file);
    if (!hash) fileHashes.set(file, (hash = digest(file).catch(() => null)));
    return hash;
  }
  if (!ref || !reader) return Promise.resolve(null);
  let hash = storedHashes.get(ref.id);
  if (!hash) {
    storedHashes.set(ref.id, (hash = reader.read(ref).then(digest, () => null)));
  }
  return hash;
}

/**
 * The numbers taken in the composer: those of its images, in the text or not, and of deleted
 * ones, which keep theirs while undo can bring them back.
 */
function takenNumbers({ aui, reader, deleted }: PasteContext): number[] {
  const state = aui.composer().getState();
  return [
    ...imageMarkersIn(state.text),
    ...deleted.keys(),
    ...state.attachments.flatMap((attachment) => {
      const n = composerInlineNumber(attachment, reader);
      return n === null ? [] : [n];
    }),
  ];
}

/** Where an attachment's bytes are: its file, or its stored reference. */
function attachmentSource(
  attachment: { id: string; file?: File | undefined },
  reader: AttachmentReader | null,
): AttachmentSource {
  return attachment.file ? { file: attachment.file } : { ref: reader?.composerRef(attachment.id) };
}

/** Images by number with their content hashes, but those whose bytes can't be read. */
async function withHashes(
  images: readonly [number, AttachmentSource][],
  reader: AttachmentReader | null,
): Promise<[number, string][]> {
  const hashes = await Promise.all(images.map(([, source]) => contentHash(source, reader)));
  return images.flatMap(([n], index) => {
    const hash = hashes[index];
    return hash ? [[n, hash]] : [];
  });
}

/** The composer's pasted images by number, deleted ones among them, but those `leftOut`. */
function composerImages(
  { aui, reader, deleted }: PasteContext,
  leftOut: (attachment: Attachment) => boolean,
): [number, AttachmentSource][] {
  return [
    ...[...deleted].map(([n, image]): [number, AttachmentSource] => [
      n,
      image instanceof File
        ? { file: image }
        : { ref: image.id === undefined ? undefined : reader?.composerRef(image.id) },
    ]),
    ...aui
      .composer()
      .getState()
      .attachments.flatMap((attachment): [number, AttachmentSource][] => {
        const n = composerInlineNumber(attachment, reader);
        return n === null || leftOut(attachment) ? [] : [[n, attachmentSource(attachment, reader)]];
      }),
  ];
}

/** An image just pasted: its number, what was attached for it, and that being added. */
type PastedImage = { n: number; image: File | CreateAttachment; added: Promise<void> };

/** What was attached for pasted images not yet matched: not yet images the composer has. */
const unmatched = new Set<File | CreateAttachment>();

/** Whether `attachment` failed to store (it can't be sent). */
function failed(attachment: Attachment): boolean {
  return attachment.status.type === "incomplete";
}

/** Whether `attachment` is what was attached for one of `images`. */
function isOneOf(attachment: Attachment, images: Iterable<File | CreateAttachment>): boolean {
  for (const image of images) {
    if (image instanceof File ? attachment.file === image : attachment.id === image.id) return true;
  }
  return false;
}

/**
 * Once pasted images are attached, makes each that is an image the composer has already (the
 * same bytes) that image: its chips take that image's number and its own attachment goes, so
 * the image is attached and sent once however often it is pasted. Until then (the bytes take a
 * moment to store and hash) it is an image of its own, where it was pasted. `pasted` is the
 * editor as the paste left it.
 */
function mergeSameImages(
  images: readonly PastedImage[],
  pasted: Promise<EditorState>,
  context: PasteContext,
) {
  if (images.length === 0) return;
  for (const { image } of images) unmatched.add(image);
  afterMerges(async () => {
    const { aui, reader, editor, formatter } = context;
    const before = await pasted;
    await Promise.allSettled(images.map(({ added }) => added));
    const composer = aui.composer();
    // The pasted images' own attachments, if still in the composer (not sent meanwhile, say):
    // known by what was attached, not by number, which another image may have taken since.
    const pastedImages = images.map(({ image }) => image);
    const isPasted = (attachment: Attachment) => isOneOf(attachment, pastedImages);
    for (const { image } of images) unmatched.delete(image);
    // One that failed to store can't be sent, so it is matched with none.
    const [own, others] = await Promise.all([
      withHashes(
        composer
          .getState()
          .attachments.filter((attachment) => isPasted(attachment) && !failed(attachment))
          .flatMap((attachment): [number, AttachmentSource][] => {
            const n = composerInlineNumber(attachment, reader);
            return n === null ? [] : [[n, attachmentSource(attachment, reader)]];
          }),
        reader,
      ),
      // Not those pasted since, still to be matched: they are matched with these, after.
      withHashes(
        composerImages(
          context,
          (attachment) =>
            isPasted(attachment) || isOneOf(attachment, unmatched) || failed(attachment),
        ),
        reader,
      ),
    ]);
    const same = sameImages(own, others);
    const merged = composer
      .getState()
      .attachments.filter(isPasted)
      .flatMap((attachment) => {
        const m = same.get(composerInlineNumber(attachment, reader) ?? 0);
        return m === undefined ? [] : [{ attachment, m }];
      });
    if (merged.length === 0) return;
    const numbers = new Map(
      merged.map(({ attachment, m }) => [composerInlineNumber(attachment, reader) ?? 0, m]),
    );
    // Nothing edited since the paste, these attachments' numbers can't come back with undo, so
    // they go and their numbers are free again. Else undo could bring them back, so once their
    // chips are renumbered InlineImageSync keeps them for it.
    if (editor.getEditorState() === before) {
      await Promise.all(
        merged.map(({ attachment }) => composer.attachment({ id: attachment.id }).remove()),
      );
    }
    // Part of the paste (or the last edit since), for undo; and if the field has been left
    // meanwhile, its caret isn't put back, which would take the focus back from elsewhere.
    const focused = editor.getRootElement()?.contains(document.activeElement) ?? false;
    editor.update(() => $renumberImageChips(numbers, formatter), {
      discrete: true,
      tag: focused ? HISTORY_MERGE_TAG : [HISTORY_MERGE_TAG, SKIP_DOM_SELECTION_TAG],
    });
  });
}

/** Runs `update` on `editor`: the editor as it left it. */
function updated(
  editor: LexicalEditor,
  update: () => void,
  options: { tag?: string },
): Promise<EditorState> {
  return new Promise((resolve) =>
    editor.update(update, {
      ...options,
      discrete: true,
      onUpdate: () => resolve(editor.getEditorState()),
    }),
  );
}

/** Gives each image chip numbered `k` in `numbers` the number `numbers` maps it to. */
function $renumberImageChips(
  numbers: ReadonlyMap<number, number>,
  formatter: Unstable_DirectiveFormatter,
) {
  for (const { node } of $dfs()) {
    if (!$isDirectiveNode(node)) continue;
    const m = numbers.get(imageMarkerAt(node.getTextContent(), 0)?.n ?? 0);
    if (m !== undefined) node.replace($createDirectiveNodeWithFormatter(imageChip(m), formatter));
  }
}

/** Pastes image data at the caret as chips, numbered on from the composer's images. */
function pasteImages(files: readonly File[], context: PasteContext) {
  const first = nextImageNumber(takenNumbers(context));
  const numbers = files.map((_, index) => first + index);
  // The chips go in first: an image's attachment counts once its chip is in the text.
  const pasted = updated(context.editor, () => $insertImageChips(numbers, context.formatter), {});
  const composer = context.aui.composer();
  const images = files.map((file, index) => {
    const n = first + index;
    const image = inlineImageFile(file, n);
    return { n, image, added: composer.addAttachment(image) };
  });
  mergeSameImages(images, pasted, context);
}

/** Whether a deleted chip's image is `source` (the chip was cut, and is pasted back). */
function isImage(
  image: File | CreateAttachment,
  source: AttachmentSource | undefined,
  reader: AttachmentReader | null,
): boolean {
  if (!source) return false;
  if (image instanceof File) return image === source.file;
  const ref = image.id === undefined ? undefined : reader?.composerRef(image.id);
  return ref !== undefined && ref.id === source.ref?.id;
}

/** `source` as the attachment for pasted image `n`: its stored reference, else its file. */
function pastedImage(
  source: AttachmentSource,
  n: number,
  adapter: BlobAttachmentAdapter | null,
): File | CreateAttachment | null {
  if (source.ref && adapter) return adapter.adopt({ ...source.ref, inline: n });
  if (!source.file) return null;
  const { name, type, lastModified } = source.file;
  return inlineImageFile(new File([source.file], name, { type, lastModified }), n);
}

/**
 * Pastes text with `[Image #n]` markers in it: the chips cut or copied with it (`copied`) come
 * back as chips with their images, numbered anew where their number is taken (by an image in
 * the composer, or one undo could bring back). A marker whose image didn't come along becomes
 * plain words. A copied image the composer has already then takes that image's number.
 */
function pasteWithImages(
  text: string,
  copied: ReadonlyMap<number, AttachmentSource>,
  context: PasteContext,
) {
  const { editor, formatter, aui, reader, adapter, deleted } = context;
  const composer = aui.composer();
  // A cut chip pasted back takes its number back from undo; it is attached again below.
  const back = [...deleted].filter(([n, image]) => isImage(image, copied.get(n), reader));
  for (const [n] of back) deleted.delete(n);
  const numbers = pastedImageNumbers(
    [...imageMarkersIn(text)].filter((n) => copied.has(n)),
    takenNumbers(context),
  );
  // The chips go in first: an image's attachment counts once its chip is in the text.
  const pasted = updated(
    editor,
    () => $insertWithImageChips(renumberImages(text, numbers), formatter),
    { tag: PASTE_TAG },
  );
  const images: PastedImage[] = [];
  for (const [n, next] of numbers) {
    const source = copied.get(n);
    const image = source && pastedImage(source, next, adapter);
    if (image) images.push({ n: next, image, added: composer.addAttachment(image) });
  }
  mergeSameImages(images, pasted, context);
}

/** Puts pasted `text` at the caret, its `[Image #n]` markers as chips. */
function $insertWithImageChips(text: string, formatter: Unstable_DirectiveFormatter) {
  for (const piece of splitAtImages(text)) {
    const selection = $getSelection();
    if (!$isRangeSelection(selection)) return;
    if (typeof piece === "string") selection.insertRawText(piece);
    else selection.insertNodes([$createDirectiveNodeWithFormatter(imageChip(piece), formatter)]);
  }
}

/**
 * Puts a chip for each pasted image `numbers` at the caret (or at the end, if the field never
 * had one), a space after them unless one follows already.
 */
function $insertImageChips(numbers: readonly number[], formatter: Unstable_DirectiveFormatter) {
  let selection = $getSelection();
  if (!$isRangeSelection(selection)) {
    $getRoot().selectEnd();
    selection = $getSelection();
  }
  if (!$isRangeSelection(selection)) return;
  const chips: LexicalNode[] = numbers.map((n) =>
    $createDirectiveNodeWithFormatter(imageChip(n), formatter),
  );
  selection.insertNodes(chips);
  const after = $getSelection();
  if (!$isRangeSelection(after)) return;
  const node = after.anchor.getNode();
  const next = $isTextNode(node) ? node.getTextContent()[after.anchor.offset] : undefined;
  if (!next || !/\s/.test(next)) after.insertText(" ");
}

/**
 * Keeps pasted images and their chips together: deleting a chip removes its image, and
 * undoing that brings the image back. A chip whose image isn't known (a marker typed by
 * hand) stays a label.
 */
function InlineImageSync({ deleted }: { deleted: DeletedImages }) {
  const aui = useAui();
  const reader = useContext(AttachmentReaderContext);
  const adapter = useContext(ComposerTargetContext)?.queue.attachments;
  const text = useAuiState((s) => s.composer.text);
  const attachments = useAuiState((s) => s.composer.attachments);
  // Numbers whose chip has been in the text.
  const seen = useRef(new Set<number>());
  const before = useRef("");
  useEffect(() => {
    const composer = aui.composer();
    // Sent or cleared all at once: what was deleted before can't come back.
    if (text === "" && attachments.length === 0 && before.current !== "") {
      seen.current.clear();
      deleted.clear();
    }
    before.current = text;
    const markers = imageMarkersIn(text);
    const present = new Set<number>();
    for (const attachment of attachments) {
      const n = composerInlineNumber(attachment, reader);
      if (n === null) continue;
      present.add(n);
      if (markers.has(n) || !seen.current.has(n)) continue;
      seen.current.delete(n);
      const ref = attachment.file ? undefined : reader?.composerRef(attachment.id);
      const again = attachment.file ?? (ref && adapter?.adopt(ref));
      if (again) deleted.set(n, again);
      void composer.attachment({ id: attachment.id }).remove();
    }
    for (const n of markers) {
      seen.current.add(n);
      const again = deleted.get(n);
      if (present.has(n) || !again) continue;
      deleted.delete(n);
      void composer.addAttachment(again);
    }
  }, [aui, reader, adapter, deleted, text, attachments]);
  return null;
}
