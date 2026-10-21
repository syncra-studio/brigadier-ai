import type { FC } from "react";

import { useViewConversation } from "@/app/conversation/viewContext";
import { Branch } from "@/components/icons";
import { LabeledDivider } from "@/components/shared";
import { openConversation } from "@/state/actions";
import { useApp } from "@/state/store";

/** "⑂ Continued from chat" under the answer a fork was made from: back to where it came from. */
export const ContinuedFrom: FC<{ forkId: string | null }> = ({ forkId }) => {
  const origin = useViewConversation()?.forkedFrom ?? null;
  const source = useApp((s) => (origin ? s.conversations[origin.conversationId] : undefined));
  if (!origin || !forkId || origin.messageId !== forkId) return null;
  return (
    <LabeledDivider>
      {source ? (
        <button
          type="button"
          onClick={() => openConversation(source.id)}
          className="text-link hover:text-link/80 flex items-center gap-1.5"
        >
          <Branch className="size-icon-sm" />
          Continued from chat
        </button>
      ) : (
        <span className="flex items-center gap-1.5">
          <Branch className="size-icon-sm" />
          Continued from chat
        </span>
      )}
    </LabeledDivider>
  );
};
