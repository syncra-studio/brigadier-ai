import type { LexicalEditor } from "@/components/views/page-editor";
import { useEffect, useRef, useState } from "react";

import { PageFileMenu } from "@/app/side-panel/pages/PageFileMenu";
import { PageInsertMenu } from "@/app/side-panel/pages/PageInsertMenu";
import { ShowPages } from "@/app/side-panel/pages/ShowPages";
import { TooltipIconButton } from "@/components/controls";
import { ConfirmDialog, DropdownMenu, DropdownMenuTrigger } from "@/components/overlays";
import { ChevronDown, Document, SquarePlus } from "@/components/icons";
import { pageDelete, saveCopy } from "@/ipc/client";
import type { PageContent } from "@/ipc/generated";
import { pageEditing } from "@/state/pageEditing";
import { dropPageTabs, openPageTab } from "@/state/sessionTabs";

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
  const renaming = useRef(false);
  useEffect(() => {
    if (!active) return;
    const key = (event: KeyboardEvent) => {
      const command = mac ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey;
      if (command && event.altKey && !event.shiftKey && event.code === "KeyE") { event.preventDefault(); setListing(true); }
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
        <PageFileMenu
          mac={mac}
          onCloseAutoFocus={(event) => {
            if (!renaming.current) return;
            event.preventDefault();
            renaming.current = false;
            // The closing menu releases its focus trap after this callback returns.
            requestAnimationFrame(onRename);
          }}
          onRename={() => { renaming.current = true; }}
          onShowPages={() => setListing(true)}
          onCopy={copy}
          onDownload={download}
          onDelete={() => setDeleting(true)}
        />
      </DropdownMenu>
      <div className="page-capsule">
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <TooltipIconButton tooltip="Insert" size="icon-md" className="rounded-full"><SquarePlus /></TooltipIconButton>
          </DropdownMenuTrigger>
          <PageInsertMenu editor={editor} />
        </DropdownMenu>
      </div>
      {notice && <span role="status" className="page-notice">{notice}</span>}
      <ShowPages open={listing} onOpenChange={setListing} onPick={(page) => { setListing(false); openPageTab(conversationId, page.id, page.title); }} />
      <ConfirmDialog
        open={deleting}
        onOpenChange={setDeleting}
        title="Delete this page?"
        description={`“${content.title || "Untitled page"}” is removed from this Mac. This can't be undone.`}
        confirm="Delete"
        destructive
        onConfirm={() => {
          setDeleting(false);
          pageDelete(pageId).then(() => {
            pageEditing().forget(pageId);
            dropPageTabs(pageId);
          }, (error: unknown) => setNotice(`Could not delete: ${error instanceof Error ? error.message : String(error)}`));
        }}
      />
    </div>
  );
}
