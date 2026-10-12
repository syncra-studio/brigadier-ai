/**
 * What is left of the old plain-text "New file" drafts, kept in local storage under their tab's
 * ID: read once to move them into pages (`pageTabs.ts`), then removed.
 */

const PREFIX = "brigadier.document.";

export function documentText(id: string): string {
  try { return localStorage.getItem(PREFIX + id) ?? ""; } catch { return ""; }
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

/** Drafts no tab is waiting to move into a page. */
export function pruneDocumentDrafts(ids: Set<string>): void {
  try {
    for (const key of Object.keys(localStorage)) {
      if (!key.startsWith(PREFIX)) continue;
      const id = key.slice(PREFIX.length).replace(/\.saved$/, "");
      if (!ids.has(id)) localStorage.removeItem(key);
    }
  } catch (error) { console.error("Could not remove abandoned document drafts", error); }
}
