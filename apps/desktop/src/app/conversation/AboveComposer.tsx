import { type FC, useContext } from "react";

import { ArchivedBar } from "@/app/conversation/ArchivedBar";
import { QuotaWaitStrip } from "@/app/conversation/cards/ResumesAtCard";
import { useAction } from "@/app/conversation/useAction";
import { ViewContext } from "@/app/conversation/viewContext";
import { restore } from "@/state/actions";

/** One recovery notice above the composer; archived conversations keep their Restore action. */
export const AboveComposer: FC = () => {
  const { conversation } = useContext(ViewContext);
  const action = useAction();
  if (!conversation) return null;
  if (conversation.lifecycle !== "archived") return <QuotaWaitStrip conversationId={conversation.id} />;
  return (
    <ArchivedBar
      error={action.error}
      busy={action.busy}
      onRestore={() => action.run(() => restore(conversation.id))}
    />
  );
};
