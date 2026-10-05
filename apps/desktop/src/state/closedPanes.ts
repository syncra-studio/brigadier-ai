/** The last closed terminal/page, for the shared reopen shortcut, by conversation id (or
 * "home" for Home's terminals). */
const closed: { conversationId: string; kind: "terminal" | "browser" }[] = [];
export function notePaneClose(
  conversationId: string,
  kind: "terminal" | "browser",
): void {
  closed.push({ conversationId, kind });
}
export function takePaneClose(
  conversationId: string,
): "terminal" | "browser" | null {
  const index = closed.findLastIndex(
    (entry) => entry.conversationId === conversationId,
  );
  if (index < 0) return null;
  return closed.splice(index, 1)[0]!.kind;
}
