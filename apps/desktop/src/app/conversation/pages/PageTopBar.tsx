import { ChevronDown, Copy, Document, Download, EditPencil, SquarePlus, Trash } from "@openai/apps-sdk-ui/components/Icon";
import type { LexicalEditor } from "lexical";
import { useEffect, useState } from "react";

import { shortcutLabel } from "@/app/conversation/SidePanel";
import { TooltipIconButton } from "@/components/assistant-ui/tooltip-icon-button";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuShortcut, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { pageDelete, pageList, saveCopy } from "@/ipc/client";
import type { PageContent, PageSummary } from "@/ipc/generated";
import { pageEditing } from "@/state/pageEditing";
import { dropPageTabs, openPageTab } from "@/state/sessionTabs";

import { BLOCKS, applyBlock } from "./pageBlocks";
import { BlockRow } from "./PageTools";
import { when } from "../NewTabTools";

/** A page as one Markdown document: its title as the top heading, then its body. */
export function pageAsMarkdown(content: PageContent): string {
  const title = [content.emoji, content.title].filter(Boolean).join(" ");
  return title ? `# ${title}\n\n${content.body}` : content.body;
}

/** The file name Download suggests. */
export function downloadName(title: string): string {
  const name = title.replace(/[\\/:*?"<>|\n]+/g, " ").trim().slice(0, 120);
  return `${name || "Untitled page"}.md`;
}

/** The page's bar: the File menu and Insert. */
export function PageTopBar({ conversationId, pageId, content, editor, mac, active, onRename }: {
  conversationId: string;
  pageId: string;
  content: PageContent;
  editor: () => LexicalEditor | null;
  mac: boolean;
  active: boolean;
  onRename: () => void;
}) {
  const [listing, setListing] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  useEffect(() => {
    if (!active) return;
    const key = (event: KeyboardEvent) => {
      const command = mac ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey;
      if (command && event.shiftKey && !event.altKey && event.code === "KeyE") { event.preventDefault(); setListing(true); }
    };
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  }, [active, mac]);
  useEffect(() => {
    if (!notice) return;
    const timer = setTimeout(() => setNotice(null), 2000);
    return () => clearTimeout(timer);
  }, [notice]);
  const copy = () => {
    navigator.clipboard.writeText(pageAsMarkdown(content)).then(() => setNotice("Copied as Markdown"), (error: unknown) => setNotice(`Could not copy: ${String(error)}`));
  };
  const download = () => {
    saveCopy(pageAsMarkdown(content), downloadName(content.title)).then(
      (path) => { if (path) setNotice("Downloaded"); },
      (error: unknown) => setNotice(`Could not download: ${error instanceof Error ? error.message : String(error)}`),
    );
  };
  return (
    <div data-slot="page-top-bar" className="page-top-bar">
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <button type="button" aria-label="File menu" className="page-capsule page-file-button">
            <Document aria-hidden className="size-icon-sm" /><ChevronDown aria-hidden className="size-icon-sm" />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="page-menu page-file-menu">
          <DropdownMenuItem className="page-menu-item" onSelect={() => setListing(true)}>
            <Document aria-hidden /><span className="flex-1">Show pages</span><DropdownMenuShortcut>{shortcutLabel("⇧⌘E", mac)}</DropdownMenuShortcut>
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem className="page-menu-item" onSelect={onRename}><EditPencil aria-hidden />Rename</DropdownMenuItem>
          <DropdownMenuItem className="page-menu-item" onSelect={copy}><Copy aria-hidden />Copy as Markdown</DropdownMenuItem>
          <DropdownMenuItem className="page-menu-item" onSelect={download}><Download aria-hidden />Download</DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem className="page-menu-item" variant="destructive" onSelect={() => setDeleting(true)}><Trash aria-hidden />Delete</DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      <div className="page-capsule">
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <TooltipIconButton tooltip="Insert" size="icon-md" className="rounded-full"><SquarePlus /></TooltipIconButton>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="start" className="page-menu page-insert-list" aria-label="Insert options"
            onCloseAutoFocus={(event) => { const current = editor(); if (current) { event.preventDefault(); current.focus(); } }}>
            <div className="page-menu-label">Basic</div>
            {BLOCKS.filter((block) => block.kind !== "paragraph").map((block) => (
              <DropdownMenuItem key={block.kind} className="page-menu-item" onSelect={() => { const current = editor(); if (current) applyBlock(current, block.kind); }}>
                <BlockRow block={block} />
              </DropdownMenuItem>
            ))}
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
      {notice && <span role="status" className="page-notice">{notice}</span>}
      <ShowPages open={listing} onOpenChange={setListing} onPick={(page) => { setListing(false); openPageTab(conversationId, page.id, page.title); }} />
      <Dialog open={deleting} onOpenChange={setDeleting}>
        <DialogContent showCloseButton={false}>
          <DialogHeader>
            <DialogTitle>Delete this page?</DialogTitle>
            <DialogDescription>“{content.title || "Untitled page"}” is removed from this Mac. This can't be undone.</DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="ghost" onClick={() => setDeleting(false)}>Cancel</Button>
            <Button variant="destructive" onClick={() => {
              setDeleting(false);
              pageDelete(pageId).then(() => {
                pageEditing().forget(pageId);
                dropPageTabs(pageId);
              }, (error: unknown) => setNotice(`Could not delete: ${error instanceof Error ? error.message : String(error)}`));
            }}>Delete</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}

/** Every page, the most recent first, narrowed by what is typed. */
function ShowPages({ open, onOpenChange, onPick }: { open: boolean; onOpenChange: (open: boolean) => void; onPick: (page: PageSummary) => void }) {
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
