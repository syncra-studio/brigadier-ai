import { Button } from "@/components/controls";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/overlays";
import { DotsHorizontal, ExternalLink, Globe, Pencil, Trash } from "@/components/icons";
import type { Bookmark } from "@/state/browserBookmarks";
import { siteOf } from "@/state/browserData";
import { openLink } from "@/state/links";

/** One bookmark: its title and site (opens it) and a menu. */
export function BrowserBookmarksRow({
  bookmark,
  onEdit,
  onRemove,
}: {
  bookmark: Bookmark;
  onEdit: () => void;
  onRemove: () => void;
}) {
  const site = siteOf(bookmark.url);
  return (
    <li data-slot="bookmark-row" className="flex min-h-11 items-center gap-3 py-1.5">
      <Globe aria-hidden className="text-foreground/65 size-icon-md shrink-0" />
      <button
        type="button"
        title={bookmark.url}
        onClick={() => void openLink(bookmark.url)}
        className="text-label flex min-w-0 flex-1 items-baseline gap-2 text-start"
      >
        <span className="truncate">{bookmark.title || site}</span>
        {bookmark.title && <span className="text-foreground/65 shrink-0 text-xs">{site}</span>}
      </button>
      <DropdownMenu modal={false}>
        <DropdownMenuTrigger asChild>
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            aria-label={`More for ${bookmark.title || site}`}
            className="text-foreground/65 shrink-0"
          >
            <DotsHorizontal />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          <DropdownMenuItem onSelect={() => void openLink(bookmark.url)}>
            <ExternalLink />
            Open
          </DropdownMenuItem>
          <DropdownMenuItem onSelect={onEdit}>
            <Pencil />
            Edit
          </DropdownMenuItem>
          <DropdownMenuItem variant="destructive" onSelect={onRemove}>
            <Trash />
            Remove bookmark
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </li>
  );
}
