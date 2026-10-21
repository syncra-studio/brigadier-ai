import { useApp } from "@/state/store";

/** A conversation's title while it exists, else its id. */
export function ConversationName({ id }: { id: string }) {
  const title = useApp((s) => s.conversations[id]?.title);
  return <span title={id}>{title ?? id}</span>;
}
