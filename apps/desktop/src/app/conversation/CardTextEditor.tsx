import { AssistantRuntimeProvider, ComposerPrimitive, useAui, useAuiState, useExternalStoreRuntime, type ThreadMessage } from "@/components/conversation/kit";
import { useLexicalComposerContext } from "@lexical/react/LexicalComposerContext";
import { $getRoot, $getSelection, $isRangeSelection, COMMAND_PRIORITY_NORMAL, DROP_COMMAND, KEY_ARROW_UP_COMMAND, KEY_ESCAPE_COMMAND, KEY_ENTER_COMMAND, type LexicalEditor } from "lexical";
import { mergeRegister } from "@lexical/utils";
import { type Ref, type RefObject, useContext, useEffect, useEffectEvent, useImperativeHandle, useLayoutEffect, useMemo, useRef, useState } from "react";

import { BlobAttachmentAdapter, ComposerAttachmentsContext, uploading } from "@/app/conversation/attachments";
import { useComposerDraft } from "@/app/conversation/composerDraft";
import { ComposerTargetContext } from "@/app/conversation/composerTarget";
import { MentionMemory, Mentions, mentionsIn } from "@/app/conversation/Mentions";
import ComposerEditor from "@/components/conversation/ComposerEditor";
import { AttachmentReaderContext } from "@/components/conversation/elements/attachment-tile";
import { request } from "@/ipc/client";
import { readAttachment } from "@/state/actions";
import { useBoard } from "@/state/board";

export type CardTextHandle = { focus: () => void; answer: () => Promise<string> };

export type CardTextEditorProps = {
  ref?: Ref<CardTextHandle>;
  value: string;
  onTyped: (text: string) => void;
  onSubmit: () => void;
  onUp: () => void;
  onEscape: () => void;
  onFocus: () => void;
  placeholder: string;
  disabled: boolean;
  draftScope?: string | undefined;
};

/** A card owns a composer runtime: its chips, clipboard and undo never edit the main message. */
export function CardTextEditor(props: CardTextEditorProps) {
  const [attachments, memory] = useMemo(() => [new BlobAttachmentAdapter(), new MentionMemory()], []);
  const runtime = useExternalStoreRuntime<ThreadMessage>({ messages: [], isDisabled: props.disabled, adapters: { attachments }, onNew: async () => {} });
  const reader = useMemo(() => ({ read: readAttachment, composerRef: (id: string) => attachments.refOf(id) }), [attachments]);
  return <AssistantRuntimeProvider runtime={runtime}>
    <ComposerAttachmentsContext.Provider value={attachments}>
      <AttachmentReaderContext.Provider value={reader}>
        <CardField {...props} attachments={attachments} memory={memory} />
      </AttachmentReaderContext.Provider>
    </ComposerAttachmentsContext.Provider>
  </AssistantRuntimeProvider>;
}

function CardField({ attachments, memory, ...props }: CardTextEditorProps & { attachments: BlobAttachmentAdapter; memory: MentionMemory }) {
  const target = useContext(ComposerTargetContext);
  const conversationId = useBoard((s) => s.board?.conversationId ?? null);
  const aui = useAui();
  const editorRef = useRef<LexicalEditor | null>(null);
  const focusRequested = useRef(false);
  const text = useAuiState((s) => s.composer.text);
  const changed = useEffectEvent(props.onTyped);
  const previous = useRef(props.value);
  const first = useRef(true);
  const [ready, setReady] = useState(false);
  // useComposerDraft restores before this effect; initialText also supports cards without a scope.
  useLayoutEffect(() => {
    const composer = aui.composer();
    if ((first.current && composer.getState().text === "") || previous.current !== props.value) composer.setText(props.value);
    previous.current = props.value;
    first.current = false;
    // Mount Lexical after restoring the external runtime; its initial empty update must not erase the draft.
    // oxlint-disable-next-line react/set-state-in-effect
    setReady(true);
  }, [aui, props.value]);
  useEffect(() => {
    // A restored or dictated value can reach the runtime before this render's snapshot.
    if (ready && aui.composer().getState().text === text) changed(text);
  }, [aui, text, ready]);
  useImperativeHandle(props.ref, () => ({
    focus: () => {
      if (editorRef.current) editorRef.current.focus();
      else focusRequested.current = true;
    },
    answer: async () => {
      const state = aui.composer().getState();
      if (uploading(state.attachments)) throw new Error("Files uploading…");
      const failed = state.attachments.find((attachment) => attachment.status.type === "incomplete");
      if (failed) throw new Error(`${failed.name} could not be stored; remove it and paste it again.`);
      const refs = attachments.messageRefs(state.text, state.attachments);
      const mentions = mentionsIn(state.text, target?.targets ?? [], memory.known());
      if (refs.length === 0 && mentions.length === 0) return state.text.trim();
      const owner = target?.conversation?.id ?? conversationId;
      if (!owner) throw new Error("The card's conversation is unavailable.");
      const result = await request({ method: "prepareCardAnswer", conversationId: owner, text: state.text.trim(), attachments: refs, mentions });
      return result.text;
    },
  }), [aui, attachments, memory, target, conversationId]);
  return <ComposerPrimitive.Unstable_TriggerPopoverRoot>
    {props.draftScope && <CardDraft scope={props.draftScope} attachments={attachments} memory={memory} />}
    <Mentions conversation={target?.conversation ?? null} targets={target?.targets ?? []} />
    <div className="min-w-0 flex-1" onFocus={props.onFocus}>
      {ready && <ComposerEditor placeholder={props.placeholder} autoFocus={false} card mentions={memory}>
        <CardKeys editorRef={editorRef} focusRequested={focusRequested} onSubmit={props.onSubmit} onUp={props.onUp} onEscape={props.onEscape} />
      </ComposerEditor>}
    </div>
  </ComposerPrimitive.Unstable_TriggerPopoverRoot>;
}

function CardDraft({ scope, attachments, memory }: { scope: string; attachments: BlobAttachmentAdapter; memory: MentionMemory }) {
  useComposerDraft(scope, attachments, memory);
  return null;
}

/** Mention menus take their keys first. Shift+Enter stays with Lexical's newline handling. */
function CardKeys({ onSubmit, onUp, onEscape, editorRef, focusRequested }: Pick<CardTextEditorProps, "onSubmit" | "onUp" | "onEscape"> & {
  editorRef: RefObject<LexicalEditor | null>;
  focusRequested: RefObject<boolean>;
}) {
  const [editor] = useLexicalComposerContext();
  useLayoutEffect(() => {
    editorRef.current = editor;
    if (focusRequested.current) {
      editor.focus();
    }
    return () => { editorRef.current = null; };
  }, [editor, editorRef, focusRequested]);
  const submit = useEffectEvent(onSubmit);
  const up = useEffectEvent(onUp);
  const escape = useEffectEvent(onEscape);
  useEffect(() => mergeRegister(
    editor.registerCommand(DROP_COMMAND, (event) => { event.preventDefault(); return true; }, COMMAND_PRIORITY_NORMAL),
    editor.registerCommand(KEY_ENTER_COMMAND, (event) => {
      if (!event || event.isComposing || event.shiftKey) return false;
      event.preventDefault();
      submit();
      return true;
    }, COMMAND_PRIORITY_NORMAL),
    editor.registerCommand(KEY_ESCAPE_COMMAND, (event) => {
      event.preventDefault();
      escape();
      return true;
    }, COMMAND_PRIORITY_NORMAL),
    editor.registerCommand(KEY_ARROW_UP_COMMAND, (event) => {
      const selection = $getSelection();
      if (!$isRangeSelection(selection) || !selection.isCollapsed()) return false;
      // Leave a multiline answer only at its start; elsewhere ↑ moves the caret normally.
      const before = selection.clone();
      before.anchor.set($getRoot().getKey(), 0, "element");
      if (before.getTextContent() !== "") return false;
      event.preventDefault();
      up();
      return true;
    }, COMMAND_PRIORITY_NORMAL),
  ), [editor]);
  return null;
}
