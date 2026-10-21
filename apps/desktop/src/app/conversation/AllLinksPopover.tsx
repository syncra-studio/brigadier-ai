import { hostOf, openLink } from "@/app/conversation/summaryLinks";
import { PopoverContent } from "@/components/overlays";

/** A Chat's Sources › "View all": every link in the user's messages, opened on a click. */
export function AllLinksPopover({ sources }: { sources: string[] }) {
  return (
    <PopoverContent
      align="end"
      className="flex max-h-80 w-xs flex-col overflow-y-auto"
    >
      {sources.map((url) => (
        <button
          key={url}
          type="button"
          onClick={() => openLink(url)}
          className="hover:bg-muted rounded-control flex flex-col px-2 py-1 text-start"
        >
          <span className="truncate text-sm">{hostOf(url)}</span>
          <span className="text-muted-foreground truncate text-xs">
            {url}
          </span>
        </button>
      ))}
    </PopoverContent>
  );
}
