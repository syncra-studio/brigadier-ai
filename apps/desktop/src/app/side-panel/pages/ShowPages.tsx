import { useEffect, useState } from "react";

import { Input } from "@/components/controls";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/overlays";
import { Document } from "@/components/icons";
import { pageList } from "@/ipc/client";
import type { PageSummary } from "@/ipc/generated";

import { when } from "../NewTabTools";

/** Every page, the most recent first, narrowed by what is typed. */
export function ShowPages({ open, onOpenChange, onPick }: { open: boolean; onOpenChange: (open: boolean) => void; onPick: (page: PageSummary) => void }) {
  const [pages, setPages] = useState<PageSummary[] | null>(null);
  const [query, setQuery] = useState("");
  const [at, setAt] = useState(0);
  useEffect(() => {
    if (!open) return;
    let live = true;
    pageList().then((list) => { if (live) setPages(list); }, () => { if (live) setPages([]); });
    return () => { live = false; setQuery(""); setAt(0); setPages(null); };
  }, [open]);
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  const shown = (pages ?? []).filter((page) => words.every((word) => (page.title || "Untitled page").toLowerCase().includes(word)));
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="page-list-dialog" aria-describedby={undefined}>
        <DialogHeader><DialogTitle>Pages</DialogTitle></DialogHeader>
        <Input
          aria-label="Search pages"
          placeholder="Search pages"
          value={query}
          onChange={(event) => { setQuery(event.target.value); setAt(0); }}
          onKeyDown={(event) => {
            if (event.key === "ArrowDown") { event.preventDefault(); setAt((index) => Math.min(index + 1, shown.length - 1)); }
            if (event.key === "ArrowUp") { event.preventDefault(); setAt((index) => Math.max(index - 1, 0)); }
            if (event.key === "Enter" && shown[at]) { event.preventDefault(); onPick(shown[at]); }
          }}
        />
        <ul aria-label="Pages" className="page-list">
          {pages && !shown.length && <li className="page-list-empty">{pages.length ? "No pages match." : "No pages yet."}</li>}
          {shown.map((page, index) => (
            <li key={page.id}>
              <button type="button" tabIndex={-1} aria-current={index === at || undefined} className="page-list-row" onMouseEnter={() => setAt(index)} onClick={() => onPick(page)}>
                {page.emoji ? <span aria-hidden className="page-list-emoji">{page.emoji}</span> : <Document aria-hidden className="size-icon-sm" />}
                <span className="min-w-0 flex-1 truncate">{page.title || "Untitled page"}</span>
                <span className="text-muted-foreground">{when(page.updatedMs)}</span>
              </button>
            </li>
          ))}
        </ul>
      </DialogContent>
    </Dialog>
  );
}
