import { type FC, useContext } from "react";

import { ArchivedBar } from "@/app/conversation/ArchivedBar";
import { useAction } from "@/app/conversation/useAction";
import { ViewContext } from "@/app/conversation/viewContext";
import { restore } from "@/state/actions";

/** Between the thread and the composer: the archived state. Notices sit on the composer's rail. */
export const AboveComposer: FC = () => {
  const { conversation } = useContext(ViewContext);
  const action = useAction();
  if (conversation?.lifecycle !== "archived") return null;
  return (
    <ArchivedBar
      error={action.error}
      busy={action.busy}
      onRestore={() => action.run(() => restore(conversation.id))}
    />
  );
};
