import { useState } from "react";

import type { SuggestedSite } from "@/app/side-panel/newTabSites";
import { Globe } from "@/components/icons";
import { useBrowsers } from "@/state/browsers";

/** Use an already-known site icon without making extra requests to sites on the launch page. */
export function SiteIcon({ site }: { site: SuggestedSite }) {
  const favicon = useBrowsers((s) => Object.values(s.pages).find((page) => {
    try { return new URL(page.url).origin === site.origin && page.favicon; } catch { return false; }
  })?.favicon);
  const [failed, setFailed] = useState<string | null>(null);
  return <span className="new-tab-site-icon">{favicon && favicon !== failed
    ? <img src={favicon} alt="" onError={() => setFailed(favicon)} /> : <Globe aria-hidden />}</span>;
}
