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
import { Chat, Folder } from "@openai/apps-sdk-ui/components/Icon";
import {
  $getRoot,
  $getSelection,
  $isRangeSelection,
  $isTextNode,
  COMMAND_PRIORITY_HIGH,
  COMMAND_PRIORITY_NORMAL,
  COPY_COMMAND,
  CUT_COMMAND,
  type EditorState,
  HISTORY_MERGE_TAG,
  type LexicalNode,
  KEY_ARROW_DOWN_COMMAND,
  KEY_ARROW_RIGHT_COMMAND,
  KEY_ARROW_UP_COMMAND,
  KEY_BACKSPACE_COMMAND,
  KEY_DOWN_COMMAND,
  KEY_ENTER_COMMAND,
  KEY_TAB_COMMAND,
  type LexicalEditor,
  PASTE_COMMAND,
  PASTE_TAG,
  SKIP_DOM_SELECTION_TAG,
} from "lexical";
import { useCallback, useContext, useEffect, useLayoutEffect, useMemo, useRef } from "react";

import { WorkerGlyph, useWorkerName } from "@/app/conversation/WorkerChip";
import { type BlobAttachmentAdapter, FILES_UPLOADING, readingFiles, uploading } from "@/app/conversation/attachments";
import { usePromptHistory } from "@/app/conversation/composerDraft";
import { enterAction } from "@/app/conversation/composerKeys";
import { clearBackgroundSend, markBackgroundSend } from "@/state/backgroundSend";
import { isTaskTab } from "@/state/tabStates";
import { ComposerTargetContext } from "@/app/conversation/composerTarget";
import { useImagesBlocked } from "@/app/conversation/imageInput";
import { SuggestionContext } from "@/app/conversation/suggestion";
import { IMAGES_BLOCKED } from "@/app/conversation/imageModels";
import {
  imageMarkerAt,
  imageMarkersIn,
  inlineMarkerAt,
  type InlineKind,
  markersIn,
  nextImageNumber,
  pastedImageNumbers,
  pasteMarkersIn,
  pastesInline,
  renumberImages,
  renumberPastes,
  sameImages,
  splitAtMarkers,
} from "@/app/conversation/inlineImages";
import { mentionOf } from "@/app/conversation/Mentions";
import { usePlanMode } from "@/app/conversation/PlusMenu";
import { askPausedSend } from "@/app/conversation/PausedSend";
import { chipContent, pastedText, pasteKind, structuredHtml } from "@/app/conversation/pasteContent";
import { usePullQueued } from "@/app/conversation/QueueCard";
import {
  type AttachmentReader,
  AttachmentReaderContext,
  type AttachmentSource,
  inlineImageFile,
  inlinePastedFile,
  pastedTextFile,
} from "@/components/assistant-ui/elements/attachment-tile";
import {
  type ChipKinds,
  ChipComposerInput,
  chipFormatter,
  imageChip,
  type MentionLook,
  pasteChip,
} from "@/components/assistant-ui/elements/composer-chips";
import { composerInlineNumber } from "@/components/assistant-ui/elements/inline-image";
import {
  cancelDictation,
  registerInserter,
  startDictation,
  stopDictation,
  useDictation,
} from "@/state/dictation";
import { useRichComposer } from "@/state/composerMode";
import { NEW_CHAT_SCOPE } from "@/state/drafts";
import { useApp } from "@/state/store";
import { toast } from "@/state/toasts";

export type ComposerInputProps = {
  placeholder: string;
  autoFocus: boolean;
};

/** The rich field's chips: code is a mark there, not a chip. */
const RICH_CHIPS: ChipKinds = { code: false, links: true };
/** The plain field's: code and links stay literal text; mentions, images and pastes are chips. */
const PLAIN_CHIPS: ChipKinds = { code: false, links: false };

/**
 * The text field (assistant-ui's Lexical input): grows with its text up to a quarter of the
 * window, then scrolls, the top line fading under the edge once scrolled. Mentions, links and
 * pasted images show as chips. The rich field shows Markdown as formatting and keeps its text
 * as Markdown; the plain one (Settings › General › Composer) keeps it as typed, code, Markdown
 * and links literal.
 */
export default function ComposerEditor({
  placeholder,
  autoFocus,
}: ComposerInputProps) {
  const target = useContext(ComposerTargetContext);
  const memory = target?.mentions ?? null;
  const targets = target?.targets;
  useEffect(() => memory?.setWorkers(targets ?? []), [memory, targets]);
  const rich = useRichComposer();
  const kinds = rich ? RICH_CHIPS : PLAIN_CHIPS;
  // One formatter per memory: a new one would rebuild every chip.
  const formatter = useMemo(
    () => chipFormatter((text, at) => memory?.match(text, at) ?? null, inlineMarkerAt, kinds),
    [memory, kinds],
  );
  // Images and pasted texts whose chip was deleted, by number, for undo to bring back.
  const deleted = useMemo(() => new Map<number, File | CreateAttachment>(), []);
  const deletedPastes = useMemo(() => new Map<number, File | CreateAttachment>(), []);
  const onMention = useCallback(
    (item: Unstable_TriggerItem) => {
      const mention = mentionOf(item);
      if (mention && mention.type !== "task") memory?.record(mention, item.label);
    },
    [memory],
  );
  return (
    <ChipComposerInput
      // The other field, on a switch, reads the same text afresh.
      key={rich ? "rich" : "plain"}
      rich={rich}
      kinds={kinds}
      formatter={formatter}
      mentionLook={mentionLook}
      onMention={onMention}
      placeholder={placeholder}
      autoFocus={autoFocus}
      // Esc stops only on a second press (useEscToStop).
      cancelOnEscape={false}
      // Enter is ComposerKeys': it sends, ⌥ and ⌘ the other ways.
      submitMode="none"
      aria-label="Message input"
    >
      <FocusGuard />
      <ComposerKeys formatter={formatter} deleted={deleted} deletedPastes={deletedPastes} />
      <SuggestionKeys />
      <PlanBackspace />
      <InlineSync kind="image" deleted={deleted} />
      <InlineSync kind="paste" deleted={deletedPastes} />
    </ChipComposerInput>
  );
}

/** Where the user types elsewhere: a text field, or anything in a dialog. */
const ELSEWHERE =
  "input:not([type=checkbox],[type=radio],[type=button],[type=submit],[type=reset],[type=range],[type=color],[type=file],[type=image]), textarea, select, [contenteditable=true], [contenteditable=''], [role=dialog] *";

/**
 * The field focuses itself when it loads and when a run starts, but never takes the keyboard
 * from what the user went to (Find in the chat, a dialog): that is decided as it focuses, which
 * can come well after its first render. The plain field that stood in while it loaded doesn't
 * count. (Lexical's `focus` is wrapped, so assistant-ui's own focusing asks too; in a layout
 * effect, so before the input's own focusing effect runs.)
 */
function FocusGuard() {
  const [editor] = useLexicalComposerContext();
  useLayoutEffect(() => guardFocus(editor), [editor]);
  return null;
}

/** Has `editor` focus only while the user isn't typing elsewhere; returns the undoing. */
function guardFocus(editor: LexicalEditor): () => void {
  const focus = editor.focus;
  editor.focus = (...args) => {
    const active = document.activeElement;
    const elsewhere =
      active instanceof HTMLElement &&
      !editor.getRootElement()?.contains(active) &&
      !active.matches('textarea[aria-label="Message input"]') &&
      active.matches(ELSEWHERE);
    if (!elsewhere) focus.apply(editor, args);
  };
  return () => {
    editor.focus = focus;
  };
}

/** In Plan mode, Backspace at the field's very start turns Plan mode off, as it would a chip there. */
function PlanBackspace() {
  const [editor] = useLexicalComposerContext();
  const plan = usePlanMode(useContext(ComposerTargetContext));
  const current = useRef(plan);
  useEffect(() => {
    current.current = plan;
  }, [plan]);
  useEffect(
    () =>
      editor.registerCommand(
        KEY_BACKSPACE_COMMAND,
        (event) => {
          if (!current.current.on || !$caretAtStart()) return false;
          event.preventDefault();
          current.current.set(false).catch((error: unknown) => console.error("plan mode off failed", error));
          return true;
        },
        COMMAND_PRIORITY_NORMAL,
      ),
    [editor],
  );
  return null;
}

/** Whether the caret, with nothing selected, is before everything in the field. */
function $caretAtStart(): boolean {
  const selection = $getSelection();
  if (!$isRangeSelection(selection) || !selection.isCollapsed() || selection.anchor.offset !== 0) return false;
  for (let node: LexicalNode | null = selection.anchor.getNode(); node && node !== $getRoot(); node = node.getParent()) {
    if (node.getPreviousSibling() !== null) return false;
  }
  return true;
}

/**
 * Tab, → or Enter in the empty field takes the guess at the next message it shows, unsent, as
 * a draft to edit; with a modifier each does what it does elsewhere. Above the field's own
 * Enter, which would send: an empty field has no open menu to take it first.
 */
function SuggestionKeys() {
  const [editor] = useLexicalComposerContext();
  const aui = useAui();
  const suggestion = useContext(SuggestionContext);
  const shown = useRef(suggestion);
  useEffect(() => {
    shown.current = suggestion;
  }, [suggestion]);
  useEffect(() => {
    const take = (event: KeyboardEvent | null) => {
      const guess = shown.current;
      if (!guess || !event || event.isComposing || event.shiftKey || event.altKey || event.ctrlKey || event.metaKey) return false;
      if (!aui.composer().getState().isEmpty) return false;
      event.preventDefault();
      aui.composer().setText(guess);
      return true;
    };
    return mergeRegister(
      editor.registerCommand(KEY_TAB_COMMAND, take, COMMAND_PRIORITY_HIGH),
      editor.registerCommand(KEY_ARROW_RIGHT_COMMAND, take, COMMAND_PRIORITY_HIGH),
      editor.registerCommand(KEY_ENTER_COMMAND, take, COMMAND_PRIORITY_HIGH),
    );
  }, [editor, aui]);
  return null;
}

/** A worker on its mention chip by its name; its `task-N` stays in the text it sends. */
function MentionedWorker({ taskId, label }: { taskId: string; label: string }) {
  return useWorkerName(taskId) ?? label;
}

/** A worker's glyph, a folder's or a conversation's icon on its mention chip; files keep their type's. */
const mentionLook: MentionLook = ({ directiveType, directiveId, label }) => {
  const id = directiveId.slice(directiveId.indexOf(":") + 1);
  if (directiveType === "task") {
    return { icon: <WorkerGlyph taskId={id} />, name: <MentionedWorker taskId={id} label={label} /> };
  }
  if (directiveType === "chat") return { icon: <Chat />, name: label };
  if (directiveType === "folder") return { icon: <Folder />, name: label.slice(label.lastIndexOf("/") + 1) };
  return null;
};

/**
 * The composer's keys and paste: ↑ in an empty field edits the last queued message, else
 * walks back through the conversation's prompts (↓ forward); a long paste becomes a
 * "Pasted text" chip (a card, into an empty field), as rich text copied from a page does;
 * pasted images go in the text at the caret as `[Image #n]` chips,
 * and other pasted files attach directly; chips cut or copied and pasted again bring their
 * images along, at any length; ⌃⇧D starts dictating at the caret and stops (or
 * cancels) again. The `@` and `/` menus take their keys first.
 */
function ComposerKeys({
  formatter,
  deleted,
  deletedPastes,
}: {
  formatter: Unstable_DirectiveFormatter;
  deleted: DeletedImages;
  deletedPastes: DeletedImages;
}) {
  const [editor] = useLexicalComposerContext();
  const aui = useAui();
  const reader = useContext(AttachmentReaderContext);
  const pull = usePullQueued();
  const target = useContext(ComposerTargetContext);
  const history = usePromptHistory(
    target?.mentions ?? null,
    target?.queue.attachments ?? null,
    useMemo(() => [deleted, deletedPastes], [deleted, deletedPastes]),
  );
  const adapter = target?.queue.attachments ?? null;
  const owner = target?.conversation?.id ?? NEW_CHAT_SCOPE;
  // A project's session: ⌘⏎ starts a new one in the background.
  const taskTab = target?.conversation && isTaskTab(target.conversation) ? target.conversation.id : null;
  const privateLogs = Boolean(target?.conversation?.tabChat);
  const conversationId = target?.conversation?.id ?? null;
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
  const mac = useApp((s) => s.info?.platform === "macos");
  const followUp = useApp((s) => s.settings.followUp);
  // Read when Enter is pressed, so the keys aren't registered again as images come and go.
  const blocked = useImagesBlocked();
  const imagesBlocked = useRef(blocked);
  useEffect(() => {
    imagesBlocked.current = blocked;
  }, [blocked]);
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
      copiedChips = copiedFrom($getSelection()?.getTextContent() ?? "", aui, reader);
      if (copiedChips && event instanceof ClipboardEvent) {
        event.clipboardData?.setData(COPIED_CHIPS_TYPE, copiedChips.token);
      }
      return false;
    };
    return mergeRegister(
      editor.registerCommand(COPY_COMMAND, copy, COMMAND_PRIORITY_NORMAL),
      editor.registerCommand(CUT_COMMAND, copy, COMMAND_PRIORITY_NORMAL),
      editor.registerCommand(KEY_ARROW_UP_COMMAND, arrow("ArrowUp"), COMMAND_PRIORITY_NORMAL),
      editor.registerCommand(KEY_ARROW_DOWN_COMMAND, arrow("ArrowDown"), COMMAND_PRIORITY_NORMAL),
      // Below the input's own Enter handling, so an open mention or command menu takes Enter
      // first. While an answer works, the other key sends the follow-up's other way.
      editor.registerCommand(
        KEY_ENTER_COMMAND,
        (event) => {
          if (!event || event.isComposing) return false;
          const action = enterAction(event, mac, taskTab !== null);
          if (!action) return false;
          event.preventDefault();
          // A ⌘⏎ whose send never came is not this send's.
          if (taskTab) clearBackgroundSend(taskTab);
          const thread = aui.thread().getState();
          if (action === "background" && taskTab) {
            // Not this conversation's send: it goes to a new tab (on the same model), running
            // or not.
            if (aui.composer().getState().isEmpty || refuseBlockedImages(imagesBlocked) || refuseUploading(aui, owner)) return true;
            markBackgroundSend(taskTab);
            aui.composer().send();
            return true;
          }
          if (thread.isRunning && !thread.capabilities.queue) return true;
          if (refuseBlockedImages(imagesBlocked) || refuseUploading(aui, owner)) return true;
          const send = () =>
            aui.composer().send(
              action === "other" && thread.isRunning ? { steer: followUp === "queue" } : undefined,
            );
          if (!askPausedSend(conversationId, send)) send();
          return true;
        },
        COMMAND_PRIORITY_NORMAL,
      ),
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
          const context = { editor, formatter, aui, reader, adapter, deleted, deletedPastes, privateLogs };
          // Chips cut or copied with their images and texts come back as chips, however long
          // their text.
          const copied = copiedChipsOn(event.clipboardData, text);
          if (files.length === 0) {
            // oxlint-disable-next-line no-underscore-dangle -- Lexical's only way to the editor's namespace
            const paste = pastedText(event.clipboardData, editor._config.namespace);
            if (!copied && pasteKind(paste, structuredHtml) === "chip") {
              event.preventDefault();
              pasteChipText(chipContent(paste), context);
              return true;
            }
            // The rest the field pastes itself (formatted, where it has formatting), but
            // `[Image #n]` and `[Pasted text #n]` markers, whose images and texts come along
            // or which become words.
            if (imageMarkersIn(text).size === 0 && pasteMarkersIn(text).size === 0) return false;
            event.preventDefault();
            pasteWithImages(text, copied ?? NOTHING_COPIED, context);
            return true;
          }
          event.preventDefault();
          if (pastesInline({ files, uris: event.clipboardData.getData("text/uri-list") })) {
            pasteImages(files, context);
            return true;
          }
          for (const file of files) void composer.addAttachment(file);
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
    privateLogs,
    formatter,
    deleted,
    deletedPastes,
    pull,
    history,
    owner,
    canDictate,
    dictating,
    opening,
    mac,
    followUp,
    taskTab,
    conversationId,
  ]);
  return null;
}

/** The images of deleted chips by their number, until the composer is sent or cleared. */
type DeletedImages = Map<number, File | CreateAttachment>;

/** The images and pasted texts of chips cut or copied, by number. */
type CopiedChips = {
  images: ReadonlyMap<number, AttachmentSource>;
  pastes: ReadonlyMap<number, AttachmentSource>;
};

const NOTHING_COPIED: CopiedChips = { images: new Map(), pastes: new Map() };

/**
 * The images and pasted texts of the chips last cut or copied from a composer, by number, with
 * the text that went on the clipboard and the token marking it: pasting that clipboard again,
 * in any composer, brings them along.
 */
let copiedChips: (CopiedChips & { text: string; token: string }) | null = null;

/** Where a cut or copy marks the clipboard as holding `copiedChips`' text. */
const COPIED_CHIPS_TYPE = "application/x-brigadier-images";

/** The images and pasted texts of the chips in `text` (cut or copied from the composer), if any. */
function copiedFrom(
  text: string,
  aui: ReturnType<typeof useAui>,
  reader: AttachmentReader | null,
): typeof copiedChips {
  const { attachments } = aui.composer().getState();
  const of = (kind: InlineKind) => {
    const found = new Map<number, AttachmentSource>();
    for (const n of markersIn(kind, text)) {
      const attachment = attachments.find((entry) => composerInlineNumber(entry, reader, kind) === n);
      if (attachment) found.set(n, { file: attachment.file, ref: reader?.composerRef(attachment.id) });
    }
    return found;
  };
  const images = of("image");
  const pastes = of("paste");
  return images.size + pastes.size > 0 ? { text, token: crypto.randomUUID(), images, pastes } : null;
}

/**
 * The images and pasted texts that came along with pasted `text`: those of the last cut or
 * copy, if this is still its clipboard (not the same text copied since from elsewhere).
 */
function copiedChipsOn(clipboard: DataTransfer, text: string): CopiedChips | null {
  if (copiedChips?.text !== text || clipboard.getData(COPIED_CHIPS_TYPE) !== copiedChips.token) {
    return null;
  }
  return copiedChips;
}

/** What pasting into a composer works with. */
type PasteContext = {
  privateLogs: boolean;
  editor: LexicalEditor;
  formatter: Unstable_DirectiveFormatter;
  aui: ReturnType<typeof useAui>;
  reader: AttachmentReader | null;
  adapter: BlobAttachmentAdapter | null;
  deleted: DeletedImages;
  deletedPastes: DeletedImages;
};

/** Images for a model that takes none: the message waits, as the send button says. */
function refuseBlockedImages(blocked: { current: boolean }): boolean {
  if (!blocked.current) return false;
  toast(IMAGES_BLOCKED, { tone: "error" });
  return true;
}

/** Files still being read or stored: the message waits, as the send button says. */
function refuseUploading(aui: ReturnType<typeof useAui>, owner: string): boolean {
  if (!readingFiles(owner) && !uploading(aui.composer().getState().attachments)) return false;
  toast(FILES_UPLOADING);
  return true;
}

/** Merges of pasted images, one after another: each sees the composer as the last left it. */
let merging = Promise.resolve();

function afterMerges(privateLogs: boolean, merge: () => Promise<void>) {
  merging = merging.then(merge).catch((error: unknown) => {
    if (privateLogs) console.error("tab chat merge failed");
    else console.error("merge failed", error);
  });
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
  afterMerges(context.privateLogs, async () => {
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
    const [ownHashes, otherHashes] = await Promise.all([
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
    // Upload status may change while bytes are being hashed. Match only copies that can
    // still be sent at the moment the chips are changed, including both sides of a match.
    const current = composer.getState().attachments;
    const sendable = new Set(current.filter((attachment) =>
      attachment.status.type === "requires-action" || attachment.status.type === "complete",
    ).map((attachment) => composerInlineNumber(attachment, reader)));
    const own = ownHashes.filter(([n]) => sendable.has(n));
    const others = otherHashes.filter(([n]) => sendable.has(n) || context.deleted.has(n));
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

/**
 * Whether a deleted chip's image (or pasted text) is `source` (the chip was cut, and is pasted
 * back).
 */
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

/** `source` as the attachment for pasted text `n`: its stored reference, else its file. */
function pastedCopy(
  source: AttachmentSource,
  n: number,
  adapter: BlobAttachmentAdapter | null,
): File | CreateAttachment | null {
  if (source.ref && adapter) return adapter.adopt({ ...source.ref, inline: n });
  if (!source.file) return null;
  const { name, type, lastModified } = source.file;
  return inlinePastedFile(new File([source.file], name, { type, lastModified }), n);
}

/**
 * The pasted-text numbers taken in the composer: those in its text, of its pasted texts, and
 * of deleted ones, which keep theirs while undo can bring them back.
 */
function takenPasteNumbers({ aui, reader, deletedPastes }: PasteContext): number[] {
  const state = aui.composer().getState();
  return [
    ...pasteMarkersIn(state.text),
    ...deletedPastes.keys(),
    ...state.attachments.flatMap((attachment) => {
      const n = composerInlineNumber(attachment, reader, "paste");
      return n === null ? [] : [n];
    }),
  ];
}

/**
 * Pastes text with `[Image #n]` or `[Pasted text #n]` markers in it: the chips cut or copied
 * with it (`copied`) come back as chips with their images and texts, numbered anew where their
 * number is taken (by one in the composer, or one undo could bring back). A marker whose image
 * or text didn't come along becomes plain words. A copied image the composer has already then
 * takes that image's number.
 */
function pasteWithImages(text: string, copied: CopiedChips, context: PasteContext) {
  const { editor, formatter, aui, reader, adapter, deleted, deletedPastes } = context;
  const composer = aui.composer();
  // A cut chip pasted back takes its number back from undo; it is attached again below.
  for (const [cut, from] of [[deleted, copied.images], [deletedPastes, copied.pastes]] as const) {
    const back = [...cut].filter(([n, image]) => isImage(image, from.get(n), reader));
    for (const [n] of back) cut.delete(n);
  }
  const numbers = pastedImageNumbers(
    [...imageMarkersIn(text)].filter((n) => copied.images.has(n)),
    takenNumbers(context),
  );
  const pasteNumbers = pastedImageNumbers(
    [...pasteMarkersIn(text)].filter((n) => copied.pastes.has(n)),
    takenPasteNumbers(context),
  );
  // The chips go in first: an attachment counts once its chip is in the text.
  const pasted = updated(
    editor,
    () => $insertWithChips(renumberPastes(renumberImages(text, numbers), pasteNumbers), formatter),
    { tag: PASTE_TAG },
  );
  const images: PastedImage[] = [];
  for (const [n, next] of numbers) {
    const source = copied.images.get(n);
    const image = source && pastedImage(source, next, adapter);
    if (image) images.push({ n: next, image, added: composer.addAttachment(image) });
  }
  for (const [n, next] of pasteNumbers) {
    const source = copied.pastes.get(n);
    const copy = source && pastedCopy(source, next, adapter);
    if (copy) void composer.addAttachment(copy);
  }
  mergeSameImages(images, pasted, context);
}

/** Puts pasted `text` at the caret, its `[Image #n]` and `[Pasted text #n]` markers as chips. */
function $insertWithChips(text: string, formatter: Unstable_DirectiveFormatter) {
  for (const piece of splitAtMarkers(text)) {
    const selection = $getSelection();
    if (!$isRangeSelection(selection)) return;
    if (typeof piece === "string") selection.insertRawText(piece);
    else {
      const item = piece.kind === "paste" ? pasteChip(piece.n) : imageChip(piece.n);
      selection.insertNodes([$createDirectiveNodeWithFormatter(item, formatter)]);
    }
  }
}

/** Puts a chip for each pasted image `numbers` at the caret, as `$insertChips` does. */
function $insertImageChips(numbers: readonly number[], formatter: Unstable_DirectiveFormatter) {
  $insertChips(numbers.map(imageChip), formatter);
}

/**
 * Puts `items`' chips at the caret (or at the end, if the field never had one), a space after
 * them unless one follows already.
 */
function $insertChips(items: readonly Unstable_TriggerItem[], formatter: Unstable_DirectiveFormatter) {
  let selection = $getSelection();
  if (!$isRangeSelection(selection)) {
    $getRoot().selectEnd();
    selection = $getSelection();
  }
  if (!$isRangeSelection(selection)) return;
  const chips: LexicalNode[] = items.map((item) => $createDirectiveNodeWithFormatter(item, formatter));
  selection.insertNodes(chips);
  const after = $getSelection();
  if (!$isRangeSelection(after)) return;
  const node = after.anchor.getNode();
  const next = $isTextNode(node) ? node.getTextContent()[after.anchor.offset] : undefined;
  if (!next || !/\s/.test(next)) after.insertText(" ");
}

/**
 * Pastes rich or long text as a chip at the caret, `[Pasted text #n]` numbered on from the
 * composer's pasted texts, its content attached as that text. Into an empty composer it goes
 * to the attachments row instead, as a card.
 */
function pasteChipText(
  { content, markdown }: { content: string; markdown: boolean },
  context: PasteContext,
) {
  const { editor, formatter, aui } = context;
  const composer = aui.composer();
  if (composer.getState().text.trim() === "") {
    void composer.addAttachment(pastedTextFile(content, { markdown }));
    return;
  }
  const n = nextImageNumber(takenPasteNumbers(context));
  // The chip goes in first: its text's attachment counts once its chip is in the text.
  editor.update(() => $insertChips([pasteChip(n)], formatter), {
    discrete: true,
    onUpdate: () => void composer.addAttachment(pastedTextFile(content, { markdown, n })),
  });
}

/**
 * Keeps pasted images (or, of `kind` "paste", pasted texts) and their chips together: deleting
 * a chip removes its image, and undoing that brings the image back. A chip whose image isn't
 * known (a marker typed by hand) stays a label.
 */
function InlineSync({ kind, deleted }: { kind: InlineKind; deleted: DeletedImages }) {
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
    const markers = markersIn(kind, text);
    const present = new Set<number>();
    for (const attachment of attachments) {
      const n = composerInlineNumber(attachment, reader, kind);
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
  }, [aui, reader, adapter, kind, deleted, text, attachments]);
  return null;
}
