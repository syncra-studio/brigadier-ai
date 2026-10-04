import { type Unstable_TriggerItem, useAui } from "@assistant-ui/react";
import { $createDirectiveNodeWithFormatter } from "@assistant-ui/react-lexical";
import { useLexicalComposerContext } from "@lexical/react/LexicalComposerContext";
import { mergeRegister } from "@lexical/utils";
import { Chat } from "@openai/apps-sdk-ui/components/Icon";
import {
  $getRoot,
  $getSelection,
  $isRangeSelection,
  $isTextNode,
  COMMAND_PRIORITY_NORMAL,
  KEY_ARROW_DOWN_COMMAND,
  KEY_ARROW_UP_COMMAND,
  KEY_DOWN_COMMAND,
  PASTE_COMMAND,
} from "lexical";
import { useCallback, useContext, useEffect, useMemo } from "react";
import { flushSync } from "react-dom";

import { ComposerImage, InlineImagePlugin } from "@/app/conversation/InlineImagePlugin";
import { IMAGE_UPLOAD, INLINE_IMAGE, isInlineImage } from "@/lib/inlineImages";
import { WorkerGlyph, useWorkerName } from "@/app/conversation/WorkerChip";
import { usePromptHistory } from "@/app/conversation/composerDraft";
import { ComposerTargetContext } from "@/app/conversation/composerTarget";
import { mentionOf } from "@/app/conversation/Mentions";
import { usePullQueued } from "@/app/conversation/QueueCard";
import {
  PASTE_AS_ATTACHMENT_CHARS,
  pastedTextFile,
} from "@/components/assistant-ui/elements/attachment-tile";
import {
  ChipComposerInput,
  chipFormatter,
  type MentionLook,
} from "@/components/assistant-ui/elements/composer-chips";
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
  /** One line (under an action card): it doesn't grow, it scrolls. */
  line?: boolean;
};

/**
 * The text field (assistant-ui's Lexical input): grows with its text up to a quarter of the
 * window, then scrolls, the top line fading under the edge once scrolled. Mentions, inline
 * code and links show as chips while the text stays plain.
 */
export default function ComposerEditor({
  placeholder,
  autoFocus,
  line = false,
}: ComposerInputProps) {
  const target = useContext(ComposerTargetContext);
  const memory = target?.mentions ?? null;
  const images = target?.queue.attachments.inline;
  const targets = target?.targets;
  useEffect(() => memory?.setWorkers(targets ?? []), [memory, targets]);
  // One formatter per memory: a new one would rebuild every chip.
  const formatter = useMemo(
    () => chipFormatter((text, at) => memory?.match(text, at) ?? null, images),
    [memory, images],
  );
  const look = useCallback<MentionLook>(
    (chip) => images && (chip.directiveType === INLINE_IMAGE || chip.directiveType === IMAGE_UPLOAD)
      ? { icon: null, name: <ComposerImage chip={chip} images={images} /> }
      : mentionLook(chip),
    [images],
  );
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
      mentionLook={look}
      onMention={onMention}
      line={line}
      placeholder={placeholder}
      autoFocus={autoFocus}
      // Esc stops only on a second press (useEscToStop).
      cancelOnEscape={false}
      aria-label="Message input"
    >
      {images && <InlineImagePlugin images={images} formatter={formatter} />}
      <ComposerKeys />
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
 * "Pasted text" attachment; eligible pasted images go inline, other files attach directly.
 * ⌃⇧D starts dictating at the caret and stops (or cancels) again. The `@` and `/` menus take their keys first.
 */
function ComposerKeys() {
  const [editor] = useLexicalComposerContext();
  const aui = useAui();
  const pull = usePullQueued();
  const target = useContext(ComposerTargetContext);
  const history = usePromptHistory(target?.mentions ?? null, target?.queue.attachments.inline);
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
    return mergeRegister(
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
          if (files.length === 0 && text.length < PASTE_AS_ATTACHMENT_CHARS) return false;
          event.preventDefault();
          const attach = files.length > 0 ? files : [pastedTextFile(text)];
          const composer = aui.composer();
          for (const file of attach) {
            const adapter = target?.queue.attachments;
            if (!adapter || !isInlineImage(file.type)) {
              void composer.addAttachment(file);
              continue;
            }
            const key = crypto.randomUUID();
            // Update the runtime guard before any send path can detach this draft.
            flushSync(() => adapter.inline.begin(key, file.name));
            const formatter = chipFormatter(() => null, adapter.inline);
            let selection = $getSelection();
            if (!$isRangeSelection(selection)) {
              $getRoot().selectEnd();
              selection = $getSelection();
            }
            if ($isRangeSelection(selection)) {
              selection.insertNodes([$createDirectiveNodeWithFormatter(
                { id: key, type: IMAGE_UPLOAD, label: file.name }, formatter,
              )]);
              void adapter.uploadInline(key, file);
            }
          }
          return true;
        },
        COMMAND_PRIORITY_NORMAL,
      ),
    );
  }, [editor, aui, pull, history, owner, canDictate, dictating, opening, target]);
  return null;
}
