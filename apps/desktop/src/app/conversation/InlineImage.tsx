import type { ComponentType } from "react";
import { Fragment } from "react";

import { imageTokens, inlineRefs } from "@/lib/inlineImages";
import { useAttachmentUrl } from "@/components/assistant-ui/elements/attachment-tile";
import type { AttachmentRef } from "@/ipc/generated";

/** Small enough to sit between words, with the original token kept in the message text. */
export function InlineImage({ attachment }: { attachment: AttachmentRef }) {
  const url = useAttachmentUrl({ ref: attachment });
  return url ? (
    <img src={url} alt={attachment.name} title={attachment.name} draggable={false}
      className="mx-1 inline-block size-12 rounded-md object-cover align-middle" />
  ) : (
    <span className="bg-muted mx-1 inline-block rounded-md px-2 py-1 text-xs align-middle" role="img" aria-label={attachment.name}>
      {attachment.name}
    </span>
  );
}

const PlainText = ({ text }: { text: string }) => <>{text}</>;

export function InlineImageText({ text, attachments, Text = PlainText }: {
  text: string;
  attachments: readonly AttachmentRef[];
  Text?: ComponentType<{ text: string }>;
}) {
  const refs = new Map(inlineRefs(text, attachments).map((ref) => [ref.id, ref]));
  let at = 0;
  const parts = [];
  for (const token of imageTokens(text)) {
    const ref = refs.get(token.id);
    if (!ref) continue;
    parts.push(<Fragment key={token.start}><Text text={text.slice(at, token.start)} /><InlineImage attachment={ref} /></Fragment>);
    at = token.end;
  }
  return <>{parts}<Text text={text.slice(at)} /></>;
}
