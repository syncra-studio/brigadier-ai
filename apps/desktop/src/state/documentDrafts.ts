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
  localStorage.setItem(PREFIX + id, documentText(id));
}

export function editDocument(id: string, text: string): void {
  useDocumentDrafts.setState(({ texts }) => ({ texts: { ...texts, [id]: text } }));
  clearTimeout(pending.get(id));
  pending.set(id, setTimeout(() => flushDocument(id), 250));
}

if (typeof window !== "undefined" && window.addEventListener) {
  window.addEventListener("pagehide", () => { for (const id of pending.keys()) flushDocument(id); });
}
