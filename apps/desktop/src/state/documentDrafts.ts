/**
 * What is left of the old plain-text "New file" drafts, kept in local storage under their tab's
 * ID: read once to move them into pages (`pageTabs.ts`), then removed.
 */

const PREFIX = "brigadier.document.";

export function documentText(id: string): string {
  return localStorage.getItem(PREFIX + id) ?? localStorage.getItem(PREFIX + id + ".saved") ?? "";
}

/** Whether the draft is what was last saved to its file (never saved: whether it is empty). */
export function documentIsSaved(id: string, savedPath: string | null = null): boolean {
  try {
    const saved = localStorage.getItem(PREFIX + id + ".saved");
    return (savedPath === null ? saved ?? "" : saved) === documentText(id);
  }
  catch { return false; }
}

export function discardDocument(id: string): void {
  try {
    localStorage.removeItem(PREFIX + id);
    localStorage.removeItem(PREFIX + id + ".saved");
  } catch (error) { console.error("Could not remove the document draft", error); }
}

/** Drafts may outlive their closed tab: migrate their unsaved text too, before removing keys. */
export function documentDraftIds(): string[] {
  const ids: string[] = [];
  for (let at = 0; at < localStorage.length; at += 1) {
    const key = localStorage.key(at);
    if (key?.startsWith(PREFIX) && !key.endsWith(".saved")) ids.push(key.slice(PREFIX.length));
  }
  return ids;
}
