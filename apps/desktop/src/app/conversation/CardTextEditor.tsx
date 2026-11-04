import { type Ref, useContext, useEffect, useEffectEvent, useImperativeHandle, useLayoutEffect, useMemo, useRef, useState } from "react";

import { BlobAttachmentAdapter, ComposerAttachmentsContext, uploading } from "@/app/conversation/attachments";
import { useComposerDraft } from "@/app/conversation/composerDraft";
import { ComposerInput } from "@/app/conversation/ComposerInput";
import { ComposerTargetContext } from "@/app/conversation/composerTarget";
import { MentionMemory, Mentions, mentionsIn } from "@/app/conversation/Mentions";
import { AssistantRuntimeProvider, AttachmentReaderContext, ComposerPrimitive, useAui, useAuiState, useExternalStoreRuntime, type ThreadMessage } from "@/components/conversation";
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
  const focusRef = useRef<(() => void) | null>(null);
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
      if (focusRef.current) focusRef.current();
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
      {ready && <ComposerInput placeholder={props.placeholder} autoFocus={false} mentions={memory} card={{ focusRef, focusRequested, onSubmit: props.onSubmit, onUp: props.onUp, onEscape: props.onEscape }} />}
    </div>
  </ComposerPrimitive.Unstable_TriggerPopoverRoot>;
}

function CardDraft({ scope, attachments, memory }: { scope: string; attachments: BlobAttachmentAdapter; memory: MentionMemory }) {
  useComposerDraft(scope, attachments, memory);
  return null;
}
