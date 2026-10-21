import { Button } from "@/components/controls";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/overlays";
import { DotsHorizontal, ExternalLink, Globe, Trash } from "@/components/icons";
import type { BrowserVisit } from "@/ipc/generated";
import { formatTime } from "@/lib/format";
import { siteOf } from "@/state/browserData";
import { openLink } from "@/state/links";

/** One page in Browsing history: its title and site (opens it), the time, and a menu. */
export function VisitRow({ visit, onRemove }: { visit: BrowserVisit; onRemove: () => void }) {
  const site = siteOf(visit.url);
  return (
    <li className="flex min-h-11 items-center gap-3 py-1.5">
      <Globe aria-hidden className="text-foreground/65 size-icon-md shrink-0" />
      <button
        type="button"
        title={visit.url}
        onClick={() => void openLink(visit.url)}
        className="text-label flex min-w-0 flex-1 items-baseline gap-2 text-start"
      >
        <span className="truncate">{visit.title || site}</span>
        {visit.title && <span className="text-foreground/65 shrink-0 text-xs">{site}</span>}
      </button>
      <span className="text-foreground/65 shrink-0 text-xs tabular-nums">{formatTime(visit.atMs)}</span>
      <DropdownMenu modal={false}>
        <DropdownMenuTrigger asChild>
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            aria-label={`More for ${visit.title || site}`}
            className="text-foreground/65 shrink-0"
          >
            <DotsHorizontal />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          <DropdownMenuItem onSelect={() => void openLink(visit.url)}>
            <ExternalLink />
            Open
          </DropdownMenuItem>
          <DropdownMenuItem variant="destructive" onSelect={onRemove}>
            <Trash />
            Remove from history
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </li>
  );
}
