import type { FC } from "react";

import type { Custom } from "@/app/conversation/ConversationView";
import { MessageAttachments, useAuiState } from "@/components/conversation";

/** Above a user message's text: its attachments. */
export const UserAttachments: FC = () => {
  const role = useAuiState((s) => s.message.role);
  const custom = useAuiState((s) => s.message.metadata.custom) as Custom;
  if (role !== "user" || !custom.attachments || custom.attachments.length === 0) return null;
  return <MessageAttachments attachments={custom.attachments} className="justify-end" />;
};
