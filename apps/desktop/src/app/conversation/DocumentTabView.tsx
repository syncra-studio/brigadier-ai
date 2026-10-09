import { useEffect, useRef, useState } from "react";
import { useCheckoutRoot } from "@/components/assistant-ui/file-links";
import { Button } from "@/components/ui/button";
import { saveDocument } from "@/ipc/client";
import { documentRelativePath, documentText, editDocument, flushDocument, noteDocumentSave, useDocumentDrafts } from "@/state/documentDrafts";
import { changeSessionTab, type DocumentTab } from "@/state/sessionTabs";
import { useApp } from "@/state/store";

export function DocumentTabView({ conversationId, tab, active }: { conversationId: string; tab: DocumentTab; active: boolean }) {
  const root = useCheckoutRoot();
  const text = useDocumentDrafts((s) => s.texts[tab.id] ?? documentText(tab.id));
  const mac = useApp((s) => s.info?.platform === "macos");
  const saving = useRef(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const save = async () => {
    if (saving.current) return;
    saving.current = true; setBusy(true); setError(null);
    try {
      flushDocument(tab.id);
      const written = documentText(tab.id);
      const path = await saveDocument(tab.id, written, root, tab.name);
      if (path) {
        noteDocumentSave(tab.id, written);
        changeSessionTab(conversationId, tab.id, (current) => current.kind === "document" ? {
          ...current, savedPath: path, name: path.split(/[\\/]/).pop() || "Untitled",
          relativePath: documentRelativePath(root, path),
        } : current);
      }
    } catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); }
    finally { saving.current = false; setBusy(false); }
  };
  useEffect(() => {
    if (!active) return;
    const key = (event: KeyboardEvent) => {
      if ((mac ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey) && !event.shiftKey && !event.altKey && event.code === "KeyS") {
        event.preventDefault(); void save();
      }
    };
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  });
  return <div data-slot="document-tab" className="flex min-h-0 flex-1 flex-col">
    <header className="border-border flex h-control-lg shrink-0 items-center gap-2 border-b px-4">
      <span className="min-w-0 flex-1 truncate text-sm">{tab.savedPath ?? tab.name}</span>
      <Button size="sm" variant="ghost" disabled={busy} onClick={() => void save()}>{busy ? "Saving…" : "Save"}</Button>
    </header>
    {error && <p role="alert" className="text-destructive px-4 py-2 text-sm">{error}</p>}
    <textarea aria-label={tab.name} spellCheck={false} value={text}
      onChange={(event) => editDocument(tab.id, event.target.value)}
      className="font-mono bg-background text-foreground min-h-0 flex-1 resize-none p-4 text-sm outline-none" />
  </div>;
}
