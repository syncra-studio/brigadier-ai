/** What a closed thing was: a terminal tab, a browser page, or one of a session's main tabs. */
export type ClosedPane = "terminal" | "tab";

/** The last closed terminal/page/tab, for the shared reopen shortcut, by conversation id (or
 * "home" for Home's terminals). */
const closed: { conversationId: string; kind: ClosedPane }[] = [];
export function notePaneClose(conversationId: string, kind: ClosedPane): void {
  closed.push({ conversationId, kind });
}
export function takePaneClose(conversationId: string): ClosedPane | null {
  const index = closed.findLastIndex(
    (entry) => entry.conversationId === conversationId,
  );
  if (index < 0) return null;
  return closed.splice(index, 1)[0]!.kind;
}
