import { shortcutLabel } from "@/app/side-panel/SidePanel";
import { DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuShortcut } from "@/components/overlays";
import { Copy, Document, Download, EditPencil, Trash } from "@/components/icons";

/** The page's File menu: Show pages, Rename, Copy as Markdown, Download and Delete. */
export function PageFileMenu({ mac, onCloseAutoFocus, onRename, onShowPages, onCopy, onDownload, onDelete }: {
  mac: boolean;
  onCloseAutoFocus: (event: Event) => void;
  onRename: () => void;
  onShowPages: () => void;
  onCopy: () => void;
  onDownload: () => void;
  onDelete: () => void;
}) {
  return (
    <DropdownMenuContent align="start" className="page-menu page-file-menu" onCloseAutoFocus={onCloseAutoFocus}>
      <DropdownMenuItem className="page-menu-item" onSelect={onShowPages}>
        <Document aria-hidden /><span className="flex-1">Show pages</span><DropdownMenuShortcut>{shortcutLabel("⌥⌘E", mac)}</DropdownMenuShortcut>
      </DropdownMenuItem>
      <DropdownMenuSeparator />
      <DropdownMenuItem className="page-menu-item" onSelect={onRename}><EditPencil aria-hidden />Rename</DropdownMenuItem>
      <DropdownMenuItem className="page-menu-item" onSelect={onCopy}><Copy aria-hidden />Copy as Markdown</DropdownMenuItem>
      <DropdownMenuItem className="page-menu-item" onSelect={onDownload}><Download aria-hidden />Download</DropdownMenuItem>
      <DropdownMenuSeparator />
      <DropdownMenuItem className="page-menu-item" variant="destructive" onSelect={onDelete}><Trash aria-hidden />Delete</DropdownMenuItem>
    </DropdownMenuContent>
  );
}
