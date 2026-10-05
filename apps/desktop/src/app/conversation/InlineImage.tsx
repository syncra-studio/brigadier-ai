import type { ComponentType } from "react";
import { Fragment } from "react";

import { normalizedImages } from "@/lib/inlineImages";
import { splitAtImages } from "@/app/conversation/inlineImages";
import { MessageInlineImage } from "@/components/assistant-ui/elements/inline-image";
import type { AttachmentRef } from "@/ipc/generated";

const PlainText = ({ text }: { text: string }) => <>{text}</>;

/** The AB chip in sent, queued and steered text, including both saved token formats. */
export function InlineImageText({ text, attachments, Text = PlainText }: {
  text: string;
  attachments: readonly AttachmentRef[];
  Text?: ComponentType<{ text: string }>;
}) {
  const normalized = normalizedImages(text, attachments);
  const refs = new Map(normalized.attachments.filter((ref) => ref.inline !== null).map((ref) => [ref.inline, ref]));
  return <>{splitAtImages(normalized.text).map((part, index) => (
    <Fragment key={index}>{typeof part === "string"
      ? <Text text={part} />
      : <MessageInlineImage image={refs.get(part)} n={part} />}</Fragment>
  ))}</>;
}
