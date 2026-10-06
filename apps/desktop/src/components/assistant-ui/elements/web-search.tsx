import { Search } from "@openai/apps-sdk-ui/components/Icon";
import type { ComponentProps } from "react";

import { field, mono } from "@/components/assistant-ui/elements/surfaces";
import { openUrl } from "@/ipc/client";
import { cn } from "@/lib/utils";

/**
 * A web search: the query in a pill, "Searching" while it runs, then the pages it found, each
 * a link with its site. Results show only when the search left them in the transcript.
 */

export type WebSearchResult = { title: string; url: string };

/** A page's site, without `www.`: `https://www.rust-lang.org/learn` → `rust-lang.org`. */
export function domainOf(url: string): string {
  try {
    return new URL(url).hostname.replace(/^www\./, "") || url;
  } catch {
    return url;
  }
}

/** How many results show; the rest are left out. */
const SHOWN = 8;

export function WebSearch({
  query,
  results,
  searching = false,
  className,
  ...props
}: Omit<ComponentProps<"div">, "children" | "results"> & {
  query: string;
  results: readonly WebSearchResult[];
  searching?: boolean;
}) {
  const shown = results.slice(0, SHOWN);
  return (
    <div data-slot="web-search" className={cn("flex w-full flex-col gap-2", className)} {...props}>
      <span
        className={cn(
          field,
          "text-foreground/70 rounded-capsule inline-flex w-fit max-w-full items-center gap-1.5 px-3 py-1.5 text-xs",
        )}
      >
        <Search aria-hidden className="text-foreground/40 size-icon-xs shrink-0" />
        <span className="min-w-0 truncate">{query}</span>
      </span>
      {(searching || results.length > 0) && (
        <div className="text-muted-foreground text-xs">
          {searching ? (
            <span className="shimmer">Searching</span>
          ) : (
            <span className="fade-in animate-in duration-300">
              Found {results.length} {results.length === 1 ? "page" : "pages"}
            </span>
          )}
        </div>
      )}
      {shown.length > 0 && (
        <ul className="flex flex-col">
          {shown.map((result) => {
            const domain = domainOf(result.url);
            return (
              <li key={result.url}>
                <a
                  href={result.url}
                  onClick={(event) => {
                    event.preventDefault();
                    openUrl(result.url).catch((error: unknown) => console.error("opening a link failed", error));
                  }}
                  className="fade-in animate-in hover:bg-foreground/5 rounded-control -mx-2 flex items-center gap-2.5 px-2 py-1 transition-colors duration-300"
                >
                  <span className="bg-foreground/5 text-foreground/45 rounded-control flex size-icon-sm shrink-0 items-center justify-center text-2xs font-medium">
                    {domain.charAt(0).toUpperCase()}
                  </span>
                  <span className="text-foreground/90 min-w-0 flex-1 truncate text-sm">{result.title || domain}</span>
                  <span className={cn(mono, "text-foreground/35 shrink-0")}>{domain}</span>
                </a>
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}

/**
 * The pages a search's output names: the `Links: [{"title", "url"}]` list a search tool returns,
 * else each markdown link, else each bare address. Duplicates go.
 */
export function searchResults(output: string | null | undefined): WebSearchResult[] {
  if (!output) return [];
  const found: WebSearchResult[] = [];
  const links = /Links:\s*(\[[\s\S]*?\])\s*(?:\n|$)/.exec(output);
  if (links?.[1]) {
    try {
      const parsed: unknown = JSON.parse(links[1]);
      if (Array.isArray(parsed)) {
        for (const entry of parsed) {
          if (entry && typeof entry === "object" && typeof (entry as { url?: unknown }).url === "string") {
            const { url, title } = entry as { url: string; title?: unknown };
            found.push({ url, title: typeof title === "string" ? title : "" });
          }
        }
      }
    } catch {
      // Not the list a search returns: fall back to the links in its text.
    }
  }
  if (found.length === 0) {
    for (const match of output.matchAll(/\[([^\]]+)\]\((https?:\/\/[^)\s]+)\)/g)) {
      found.push({ title: match[1] ?? "", url: match[2] ?? "" });
    }
  }
  if (found.length === 0) {
    for (const match of output.matchAll(/https?:\/\/[^\s)"'<>\]]+/g)) found.push({ title: "", url: match[0] });
  }
  const seen = new Set<string>();
  return found.filter((result) => result.url && !seen.has(result.url) && seen.add(result.url));
}
