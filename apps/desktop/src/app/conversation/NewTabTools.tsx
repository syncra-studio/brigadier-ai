import { ChatCompose, ChevronDown, Document, DotsHorizontal, Globe, Grid, Search, X } from "@openai/apps-sdk-ui/components/Icon";
import { useContext, useEffect, useState } from "react";

import { WorkspacePaneContext } from "@/app/conversation/WorkspacePane";
import { shortcutLabel } from "@/app/conversation/SidePanel";
import { DiffGlyph } from "@/components/assistant-ui/elements/diff-glyph";
import { useCheckoutRoot } from "@/components/assistant-ui/file-links";
import { TooltipIconButton } from "@/components/assistant-ui/tooltip-icon-button";
import { TerminalGlyph } from "@/components/glyphs/codicons";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Kbd } from "@/components/ui/kbd";
import { pageList } from "@/ipc/client";
import type { BrowserVisit, PageSummary } from "@/ipc/generated";
import { browserHistory } from "@/state/browserData";
import { useBrowsers } from "@/state/browsers";
import { useBrowserOn } from "@/state/links";
import { onPageChanged } from "@/state/pageChanges";
import { openPageTab, openReviewTab, replaceNewTab, useSessionTabs } from "@/state/sessionTabs";
import { useApp } from "@/state/store";
import { setTerminalOpen } from "@/state/terminalPlaces";

import { suggestedSites, type SuggestedSite } from "./newTabSites";
import "./NewTabTools.css";

/** A tool or site consumes the New tab in place; Find file waits for a file to be picked. */
export function NewTabTools({ conversationId, tabId }: { conversationId: string; tabId: string }) {
  const workspace = useContext(WorkspacePaneContext);
  const cwd = useCheckoutRoot();
  const mac = useApp((s) => s.info?.platform === "macos");
  const archived = useApp((s) => s.conversations[conversationId]?.lifecycle === "archived");
  const browserOn = useBrowserOn();
  const [history, setHistory] = useState<BrowserVisit[]>([]);
  const [dismissed, setDismissed] = useState<string[]>([]);
  useEffect(() => {
    if (!browserOn) return;
    let disposed = false;
    let revision = 0;
    const refresh = () => {
      const current = ++revision;
      void browserHistory("", "all", null).then(
        (visits) => { if (!disposed && current === revision) setHistory(Array.isArray(visits) ? visits : []); },
        () => { if (!disposed && current === revision) setHistory([]); },
      );
    };
    refresh();
    window.addEventListener("focus", refresh);
    // A retained New tab refreshes after another page finishes navigating.
    const unsubscribe = useBrowsers.subscribe((state, previous) => {
      if (Object.entries(state.pages).some(([id, page]) => !page.loading && previous.pages[id]?.loading)) refresh();
    });
    const unsubscribeTabs = useSessionTabs.subscribe((state, previous) => {
      if (state.sessions[conversationId]?.active === tabId && previous.sessions[conversationId]?.active !== tabId) refresh();
    });
    const unsubscribeSelection = useApp.subscribe((state, previous) => {
      if (state.selection !== previous.selection && state.selection.type === "conversation" && state.selection.id === conversationId) refresh();
    });
    return () => { disposed = true; unsubscribe(); unsubscribeTabs(); unsubscribeSelection(); window.removeEventListener("focus", refresh); };
  }, [browserOn, conversationId, tabId]);
  const pages = useRecentPages();
  const sites = browserOn ? suggestedSites(history, dismissed) : [];
  const tools = [
    { label: "Review changes", Icon: DiffGlyph, shortcut: "⌃⇧G", run: () => openReviewTab(conversationId, { type: "all" }, tabId) },
    { label: "Find file", Icon: Search, shortcut: "⌘P", disabled: !workspace?.enabled, run: () => workspace?.openFilesSearch(tabId) },
  ];
  return (
    <div data-slot="new-tab-page" className="new-tab-page">
      <div data-slot="new-tab-column" className="new-tab-column">
        <section aria-label="Tools" className="new-tab-section">
          <h2>Tools</h2>
          <div data-slot="new-tab-tools" className="new-tab-grid">
            <div className="new-tab-card">
              <button type="button" aria-label="Terminal" disabled={archived} onClick={() => replaceNewTab(conversationId, tabId, "terminal", cwd)} className="new-tab-tool">
                <TerminalGlyph aria-hidden /><span className="new-tab-tool-label">Terminal</span>
                <Kbd>{shortcutLabel("⌃`", mac)}</Kbd>
              </button>
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <TooltipIconButton tooltip="Terminal options" disabled={archived} className="new-tab-options shrink-0 rounded-full"><DotsHorizontal /></TooltipIconButton>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end">
                  <DropdownMenuItem onSelect={() => setTerminalOpen(`conv:${conversationId}`, true)}>Open bottom terminal</DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
            </div>
            <div className="new-tab-card">
              <button type="button" onClick={() => replaceNewTab(conversationId, tabId, "page")} className="new-tab-tool">
                <Document aria-hidden /><span className="new-tab-tool-label">New page</span>
              </button>
            </div>
            <div className="new-tab-card">
              <button type="button" aria-label="Side chat" disabled={archived} onClick={() => replaceNewTab(conversationId, tabId, "sideChat")} className="new-tab-tool">
                <ChatCompose aria-hidden /><span className="new-tab-tool-label">Side chat</span>
                <Kbd>{shortcutLabel("⌥⌘S", mac)}</Kbd>
              </button>
            </div>
            <div className="new-tab-card">
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <button type="button" className="new-tab-tool"><Grid aria-hidden /><span className="new-tab-tool-label">More tools…</span><ChevronDown aria-hidden /></button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end" data-slot="new-tab-more-tools">
                  {tools.map(({ label, Icon, shortcut, disabled, run }) => (
                    <DropdownMenuItem key={label} disabled={!!disabled} onSelect={run}>
                      <Icon aria-hidden /><span className="flex-1">{label}</span><Kbd className="font-sans">{shortcutLabel(shortcut, mac)}</Kbd>
                    </DropdownMenuItem>
                  ))}
                </DropdownMenuContent>
              </DropdownMenu>
            </div>
          </div>
        </section>
        {pages.length > 0 && (
          <section aria-label="Recents" className="new-tab-section">
            <h2>Recents</h2>
            <ul aria-label="Recent work" className="new-tab-recents">
              {pages.map((page) => (
                <li key={page.id}>
                  <button type="button" className="new-tab-recent" onClick={() => openPageTab(conversationId, page.id, page.title, tabId)}>
                    {page.emoji ? <span aria-hidden className="new-tab-recent-emoji">{page.emoji}</span> : <Document aria-hidden />}
                    <span className="new-tab-tool-label">{page.title || "Untitled page"}</span>
                    <span className="new-tab-recent-when">{when(page.updatedMs)}</span>
                  </button>
                </li>
              ))}
            </ul>
          </section>
        )}
        {sites.length > 0 && (
          <section aria-label="Suggested" className="new-tab-section">
            <h2>Suggested</h2>
            <ul className="new-tab-sites">
              {sites.map((site) => (
                <li key={site.origin} className="new-tab-site">
                  <a href={site.url} aria-label={site.title} title={site.title} className="new-tab-site-link"
                    onClick={(event) => { event.preventDefault(); replaceNewTab(conversationId, tabId, "browser", null, site.url); }}>
                    <SiteIcon site={site} /><span className="new-tab-site-title">{site.title}</span>
                  </a>
                  <TooltipIconButton tooltip={`Dismiss ${site.title}`} className="new-tab-dismiss rounded-full" onClick={() => setDismissed((origins) => [...origins, site.origin])}><X /></TooltipIconButton>
                </li>
              ))}
            </ul>
          </section>
        )}
      </div>
    </div>
  );
}

/** The pages changed most recently, kept current while the New tab shows. */
function useRecentPages(): PageSummary[] {
  const [pages, setPages] = useState<PageSummary[]>([]);
  useEffect(() => {
    let disposed = false;
    let revision = 0;
    const refresh = () => {
      const current = ++revision;
      pageList().then(
        (list) => { if (!disposed && current === revision) setPages(list.slice(0, 20)); },
        () => { if (!disposed && current === revision) setPages([]); },
      );
    };
    refresh();
    const off = onPageChanged(refresh);
    return () => { disposed = true; off(); };
  }, []);
  return pages;
}

/** "Just now", "5 min ago", "3 h ago", then the date. */
export function when(ms: number, now = Date.now()): string {
  const minutes = Math.floor((now - ms) / 60_000);
  if (minutes < 1) return "Just now";
  if (minutes < 60) return `${minutes} min ago`;
  if (minutes < 24 * 60) return `${Math.floor(minutes / 60)} h ago`;
  return new Date(ms).toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

/** Use an already-known site icon without making extra requests to sites on the launch page. */
function SiteIcon({ site }: { site: SuggestedSite }) {
  const favicon = useBrowsers((s) => Object.values(s.pages).find((page) => {
    try { return new URL(page.url).origin === site.origin && page.favicon; } catch { return false; }
  })?.favicon);
  const [failed, setFailed] = useState<string | null>(null);
  return <span className="new-tab-site-icon">{favicon && favicon !== failed
    ? <img src={favicon} alt="" onError={() => setFailed(favicon)} /> : <Globe aria-hidden />}</span>;
}
