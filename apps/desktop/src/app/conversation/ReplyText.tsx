import type { FC } from "react";

import { ReportText } from "@/app/conversation/ReportText";
import { CommentaryMessageText, MessagePrimitive, MessageText, StreamingMessageText } from "@/components/conversation";

/**
 * One reply of the block; text that still streams fades in word by word. A line between the
 * turn's steps (`commentary`) streams as prose like the answer, but shows as it arrives: only the
 * final answer is paced.
 */
export const ReplyText: FC<{ index: number; streaming: boolean; report?: boolean; commentary?: boolean }> = ({ index, streaming, report = false, commentary = false }) => (
  <MessagePrimitive.PartByIndex
    index={index}
    components={{ Text: report ? ReportText : streaming ? (commentary ? CommentaryMessageText : StreamingMessageText) : MessageText }}
  />
);
