import type { FC } from "react";

import { withoutUpdateMark } from "@/app/conversation/blocks";
import { MessageText, type TextMessagePartProps, TextMessagePartProvider } from "@/components/conversation";

/** A late note as the user reads it: its "Update:" marker, which tells it from the answer, left out. */
export const UpdateText: FC<TextMessagePartProps> = (props) => (
  <TextMessagePartProvider text={withoutUpdateMark(props.text)}>
    <MessageText {...props} text={withoutUpdateMark(props.text)} />
  </TextMessagePartProvider>
);
