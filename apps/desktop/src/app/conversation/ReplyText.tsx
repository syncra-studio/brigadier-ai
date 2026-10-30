import type { FC } from "react";

import { IsReport, ReportText } from "@/app/conversation/ReportText";
import { MessagePrimitive, TextStreams } from "@/components/conversation";

/**
 * One reply of the block; text that still streams is paced and fades in word by word, a line
 * between the turn's steps the same as the answer: not every CLI says which is which. A finished
 * answer (`report`) shows as one, in the same component it streamed in.
 */
export const ReplyText: FC<{ index: number; streaming: boolean; report?: boolean }> = ({ index, streaming, report = false }) => (
  <TextStreams.Provider value={streaming}>
    <IsReport.Provider value={report}>
      <MessagePrimitive.PartByIndex index={index} components={{ Text: ReportText }} />
    </IsReport.Provider>
  </TextStreams.Provider>
);
