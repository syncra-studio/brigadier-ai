import { Globe } from "@openai/apps-sdk-ui/components/Icon";
import { useState } from "react";

import { Spinner } from "@/components/glyphs/spinner";
import { useBrowsers } from "@/state/browsers";

import "./BrowserTabIcon.css";

/** The tab's 16px icon: spinner, a loaded 10px favicon inside its ring, then the full favicon. */
export function BrowserTabIcon({ tabId }: { tabId: string }) {
  const page = useBrowsers((s) => s.pages[tabId]);
  return <FaviconGlyph favicon={page?.favicon ?? null} loading={page?.loading ?? false} />;
}

export function FaviconGlyph({ favicon, loading }: { favicon?: string | null; loading: boolean }) {
  const [loaded, setLoaded] = useState<string | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  const usable = !!favicon && favicon !== failed && favicon === loaded;
  const phase = loading ? usable ? "loading-favicon" : "spinner-only" : "settled";
  return (
    <span data-slot="browser-tab-icon" data-phase={phase} aria-hidden="true" aria-busy={loading} className="browser-tab-icon">
      <span className="browser-tab-favicon" data-visible={usable || !loading}>
        {favicon && favicon !== failed
          ? <img src={favicon} alt="" onLoad={() => setLoaded(favicon)} onError={() => setFailed(favicon)} />
          : <Globe />}
      </span>
      {loading && <Spinner data-slot="favicon-spinner" className="browser-tab-spinner" />}
    </span>
  );
}
