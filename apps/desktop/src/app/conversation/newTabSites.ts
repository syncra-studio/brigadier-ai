import type { BrowserVisit } from "@/ipc/generated";

export type SuggestedSite = { origin: string; url: string; title: string };

/** Four recently visited sites, one per origin, keeping the most recent page's title and URL. */
export function suggestedSites(history: readonly BrowserVisit[], dismissed: readonly string[] = []): SuggestedSite[] {
  const seen = new Set(dismissed);
  const sites: SuggestedSite[] = [];
  for (const visit of history.toSorted((a, b) => b.atMs - a.atMs || b.id - a.id)) {
    let url: URL;
    try { url = new URL(visit.url); } catch { continue; }
    if (!["http:", "https:"].includes(url.protocol) || url.username || url.password || seen.has(url.origin)) continue;
    seen.add(url.origin);
    sites.push({ origin: url.origin, url: url.href, title: visit.title.trim() || url.host.replace(/^www\./, "") });
    if (sites.length === 4) break;
  }
  return sites;
}
