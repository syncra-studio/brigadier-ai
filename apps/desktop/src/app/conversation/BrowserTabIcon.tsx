import { FaviconGlyph } from "@/components/conversation";
import { useBrowsers } from "@/state/browsers";

/** The tab's 16px icon: spinner, a loaded 10px favicon inside its ring, then the full favicon. */
export function BrowserTabIcon({ tabId }: { tabId: string }) {
  const page = useBrowsers((s) => s.pages[tabId]);
  return <FaviconGlyph favicon={page?.favicon ?? null} loading={page?.loading ?? false} />;
}

