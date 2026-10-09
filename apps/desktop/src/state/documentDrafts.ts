import { create } from "zustand";

const PREFIX = "brigadier.document.";
const pending = new Map<string, ReturnType<typeof setTimeout>>();
export const useDocumentDrafts = create<{ texts: Record<string, string> }>(() => ({ texts: {} }));

export function documentText(id: string): string {
  const current = useDocumentDrafts.getState().texts[id];
  if (current !== undefined) return current;
  try { return localStorage.getItem(PREFIX + id) ?? ""; } catch { return ""; }
}

export function flushDocument(id: string): void {
  clearTimeout(pending.get(id));
  pending.delete(id);
  try { localStorage.setItem(PREFIX + id, documentText(id)); }
  catch (error) { console.error("Could not preserve the document draft", error); }
}

/** Kept outside the tabs blob, alongside the draft, so later edits survive a restart. */
export function noteDocumentSave(id: string, text: string): void {
  try { localStorage.setItem(PREFIX + id + ".saved", text); }
  catch (error) { console.error("Could not preserve the document save state", error); }
}

export function documentIsSaved(id: string): boolean {
  try { return localStorage.getItem(PREFIX + id + ".saved") === documentText(id); }
  catch { return false; }
}

export function discardDocument(id: string): void {
  clearTimeout(pending.get(id));
  pending.delete(id);
  useDocumentDrafts.setState(({ texts }) => {
    const { [id]: _gone, ...rest } = texts;
    return { texts: rest };
  });
  try {
    localStorage.removeItem(PREFIX + id);
    localStorage.removeItem(PREFIX + id + ".saved");
  } catch (error) { console.error("Could not remove the document draft", error); }
}

export function documentRelativePath(root: string | null | undefined, path: string): string | null {
  if (!root) return null;
  const directory = root.replaceAll("\\", "/").replace(/\/$/, "") + "/";
  const file = path.replaceAll("\\", "/");
  const windows = /^[A-Za-z]:\//.test(directory) || directory.startsWith("//");
  return (windows ? file.toLowerCase().startsWith(directory.toLowerCase()) : file.startsWith(directory))
    ? file.slice(directory.length) : null;
}

export function editDocument(id: string, text: string): void {
  useDocumentDrafts.setState(({ texts }) => ({ texts: { ...texts, [id]: text } }));
  clearTimeout(pending.get(id));
  pending.set(id, setTimeout(() => flushDocument(id), 250));
}

if (typeof window !== "undefined" && window.addEventListener) {
  window.addEventListener("pagehide", () => { for (const id of pending.keys()) flushDocument(id); });
}

/** Closed drafts have no persisted tab to reopen after a restart. */
export function pruneDocumentDrafts(ids: Set<string>): void {
  try {
    for (const key of Object.keys(localStorage)) {
      if (!key.startsWith(PREFIX)) continue;
      const id = key.slice(PREFIX.length).replace(/\.saved$/, "");
      if (!ids.has(id)) localStorage.removeItem(key);
    }
  } catch (error) { console.error("Could not remove abandoned document drafts", error); }
}
