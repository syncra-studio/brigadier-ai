import { useEffect, useState } from "react";

import { TooltipIconButton } from "@/components/controls";
import { Star, StarFilled } from "@/components/icons";
import { bookmarkPage, isBookmarked, unbookmarkPage, useBookmarkChanges } from "@/state/browserBookmarks";
import { tabProfileId, useProfilePlaces } from "@/state/browserProfilePlaces";
import { toast } from "@/state/toasts";

const web = (url: string) => /^https?:\/\//i.test(url);

/** The address field's star: fills when the page is bookmarked in the tab's profile, and a press adds or removes it. */
export function BookmarkStar({ tab, url, title }: { tab: string; url: string; title: string }) {
  const profile = useProfilePlaces(() => tabProfileId(tab));
  const changes = useBookmarkChanges((s) => s.count);
  const [read, setRead] = useState<{ url: string; marked: boolean } | null>(null);
  useEffect(() => {
    if (!web(url)) return;
    let current = true;
    isBookmarked(profile, url).then(
      (marked) => { if (current) setRead({ url, marked }); },
      (cause: unknown) => console.error("reading bookmarks failed", cause),
    );
    return () => { current = false; };
  }, [profile, url, changes]);
  if (!web(url)) return null;
  const marked = read?.url === url && read.marked;
  return (
    <TooltipIconButton
      type="button"
      tooltip={marked ? "Remove bookmark" : "Bookmark this page"}
      size="icon-md"
      data-slot="bookmark-star"
      aria-pressed={marked}
      className="shrink-0 rounded-full @max-md/browser-toolbar:hidden"
      onMouseDown={(event) => event.preventDefault()}
      onClick={() => {
        (marked ? unbookmarkPage(profile, url) : bookmarkPage(profile, url, title)).catch((cause: unknown) =>
          toast(cause instanceof Error ? cause.message : String(cause), { tone: "error" }),
        );
      }}
    >
      {marked ? <StarFilled /> : <Star />}
    </TooltipIconButton>
  );
}
