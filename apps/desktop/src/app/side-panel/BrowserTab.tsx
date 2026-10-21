import { type ReactNode, useEffect, useEffectEvent, useRef, useState } from "react";

import { AddressSuggestions, useAddressSuggestions } from "./AddressSuggestions";
import { type Suggestion, stepSuggestion } from "./browserSuggestions";
import { browserAddress, shownAddress, webAddress } from "./browserAddress";
import { showPage } from "../conversation/browserLayer";
import { type BrowserKey, browserKey, stepZoom } from "../conversation/browserKeys";
import { DownloadsButton, PermissionPrompt } from "./BrowserToolbarPopovers";
import { BrowserOptions } from "./BrowserOptions";
import { FindBar } from "./FindBar";
import { NavigationCapsule } from "./NavigationCapsule";
import { SiteInfo } from "./SiteInfo";

import {
  ArrowUpRight,
  CameraPhoto,
  Globe,
} from "@/components/icons";
import { Banner } from "@/components/shared";
import { Button, TooltipIconButton } from "@/components/controls";
import { browserFocus, browserGo, browserPlace, browserPrint, browserSnapshot, openUrl } from "@/ipc/client";
import type { BrowserBounds } from "@/ipc/generated";
import { closeFind, findInPage, NO_FIND, openFind, useBrowserFind } from "@/state/browserFind";
import {
  dismissBlocked,
  openPage,
  useBrowsers,
  zoomPage,
} from "@/state/browsers";
import { handBack, pageShows, useAgentPause, userUsed } from "@/state/agentPause";
import { useBrowserOn } from "@/state/links";
import { selectTab, sessionWith, useSessionTabs } from "@/state/sessionTabs";
import { useApp } from "@/state/store";
import { cn } from "@/lib/utils";
import { toast } from "@/state/toasts";
import { TabChatToolbarButton } from "./TabChat";

/**
 * A browser main tab (⌘⇧B): one page, such as the app a worker is
 * running on localhost. The page is a system webview the shell lays over this tab's area (see
 * `src-tauri/src/browser.rs`); it is made on the first address entered and hidden whenever the
 * tab is, or something of the app's (a menu, a dialog) opens over it.
 */

/** App layers that may cover the tab: dialogs always, menus and popovers where they overlap. */
const COVERS =
  "[role=dialog], [role=alertdialog], [data-radix-popper-content-wrapper]";

function covered(area: DOMRect, layers: readonly Element[]): boolean {
  return layers.some((layer) => {
    if (layer.matches("[role=dialog], [role=alertdialog]")) return true;
    // Tooltips come and go under the pointer; they don't hide the page.
    if (layer.querySelector("[role=tooltip]")) return false;
    const rect = layer.getBoundingClientRect();
    return (
      rect.right > area.left &&
      rect.left < area.right &&
      rect.bottom > area.top &&
      rect.top < area.bottom
    );
  });
}

function pageBounds(
  element: HTMLElement,
  rect = element.getBoundingClientRect(),
): BrowserBounds {
  // A native child webview paints above the app DOM. End its rectangle above the
  // floating editor, so the same editor stays accessible without a second webview.
  const composer = element
    .closest('[data-slot="pane-workspace"]')
    ?.querySelector('[data-slot="floating-composer"]')
    ?.getBoundingClientRect();
  return {
    x: rect.left,
    y: rect.top,
    width: rect.width,
    height:
      composer && composer.height > 0
        ? Math.min(rect.height, Math.max(0, composer.top - rect.top - 12))
        : rect.height,
  };
}

function failed(cause: unknown) {
  toast(cause instanceof Error ? cause.message : String(cause), {
    tone: "error",
  });
}

function openOutside(url: string) {
  openUrl(url).catch(failed);
}

const frame = () => new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));

/** Gives the keyboard back to the app's own page, which a tab's native page may hold. */
async function focusApp(): Promise<void> {
  if (!document.hasFocus()) await browserFocus(null).catch(() => {});
}

/** The agent working Browser tab `id`, as the user knows it; null for the user's own tabs. */
function driverLabel(sessions: ReturnType<typeof useSessionTabs.getState>["sessions"], id: string): string | null {
  for (const session of Object.values(sessions)) {
    const tab = session.tabs.find((each) => each.id === id);
    if (tab) return tab.kind === "browser" && tab.driver ? tab.driver.label : null;
  }
  return null;
}

export function BrowserTab({
  conversationId,
  initialUrl,
  agent = false,
  active,
  children,
  onNavigate,
}: {
  conversationId: string;
  initialUrl?: string | undefined;
  /** Opened from something an agent gave; its pages count as the agent's in the history. */
  agent?: boolean | undefined;
  active: boolean;
  children?: ReactNode;
  onNavigate?: ((url: string) => void) | undefined;
}) {
  const page = useBrowsers((s) => s.pages[conversationId]);
  const paused = useAgentPause((s) => conversationId in s.paused);
  const driver = useSessionTabs((s) => driverLabel(s.sessions, conversationId));
  const embedded = useApp((s) => s.info?.platform !== "linux");
  const mac = useApp((s) => s.info?.platform === "macos");
  /** The page lies beneath the app, which draws above it (see `browserLayer`). */
  const beneath = mac;
  const area = useRef<HTMLDivElement>(null);
  const field = useRef<HTMLInputElement>(null);
  const findInput = useRef<HTMLInputElement>(null);
  /** The page shows over the tab now (not hidden, covered or moving in). */
  const shown = useRef(false);
  const finding = useBrowserFind((s) => s.tabs[conversationId]?.open ?? false);
  const [typed, setTyped] = useState<string | null>(null);
  const [focused, setFocused] = useState(false);
  const address = useRef<HTMLDivElement>(null);
  /** The suggestion the arrow keys are on; -1: the field itself. */
  const [highlight, setHighlight] = useState(-1);
  const [suggesting, setSuggesting] = useState(true);
  const fullUrl = useApp((s) => s.settings.browserFullUrl);
  const made = page !== undefined;
  const browserOn = useBrowserOn();
  useEffect(() => {
    if (!browserOn || !embedded || !active || !initialUrl || made || !area.current) return;
    void openPage(conversationId, initialUrl, pageBounds(area.current), agent).catch(
      failed,
    );
  }, [conversationId, initialUrl, agent, made, active, embedded, browserOn]);

  // Keep the page over this tab's area, and hide it when the tab goes or is covered.
  useEffect(() => {
    const element = area.current;
    if (!page?.ready || !element) return;
    if (!active) { void browserPlace(conversationId, null).catch(() => {}); return; }
    let layers: Element[] = [];
    const collect = () => {
      layers = [...document.querySelectorAll(COVERS)];
    };
    collect();
    // Menus, popovers and dialogs mount in portals at the end of the body.
    const observer = new MutationObserver(collect);
    observer.observe(document.body, { childList: true });
    let placed = "";
    let next = 0;
    // The side panel clips its contents while it opens, closes or hides; the page, drawn over
    // the window rather than in it, shows only once the tab is wholly in view.

    const place = () => {
      const rect = element.getBoundingClientRect();
      const placedBounds = pageBounds(element, rect);
      const rendered = new DOMRect(
        placedBounds.x,
        placedBounds.y,
        placedBounds.width,
        placedBounds.height,
      );
      // Beneath the app (macOS) the page stays live under what the app opens over it; over the
      // app it hides while anything of the app's overlaps it.
      const bounds: BrowserBounds | null =
        !element.closest("[inert]") &&
        placedBounds.width > 0 &&
        placedBounds.height > 0 &&
        (beneath || !covered(rendered, layers))
          ? placedBounds
          : null;
      const key = JSON.stringify(bounds);
      if (key !== placed) {
        placed = key;
        browserPlace(conversationId, bounds).catch(() => {});
        if (beneath) showPage(conversationId, bounds);
      }
      shown.current = bounds !== null;
      pageShows(conversationId, shown.current);
      next = requestAnimationFrame(place);
    };
    next = requestAnimationFrame(place);
    return () => {
      cancelAnimationFrame(next);
      observer.disconnect();
      shown.current = false;
      pageShows(conversationId, false);
      browserPlace(conversationId, null).catch(() => {});
      if (beneath) showPage(conversationId, null);
    };
  }, [conversationId, page?.ready, active, beneath]);

  const url = page?.url ?? "";
  const loading = page?.loading === true;
  const zoom = page?.zoom ?? 1;
  // What the page does natively (find, print, zoom, a screenshot): macOS, once there is a page.
  const native = mac && page?.ready === true;

  const act = (action: Parameters<typeof browserGo>[1]) => {
    userUsed(conversationId);
    browserGo(conversationId, action).catch(failed);
  };

  /** Waits (up to 2s) for the page to show over the tab again, as once the ⋯ menu has gone. */
  const whenShown = async () => {
    for (let tries = 0; tries < 120 && !shown.current; tries++) await frame();
    await frame();
  };

  const showFind = () => {
    const { query, open } = useBrowserFind.getState().tabs[conversationId] ?? NO_FIND;
    openFind(conversationId);
    if (!open && query) void findInPage(conversationId, query, "type");
    void focusApp().then(async () => {
      await frame();
      findInput.current?.focus();
      findInput.current?.select();
    });
  };
  const hideFind = () => {
    closeFind(conversationId);
    // Back to the page, as before the find bar opened.
    if (made) browserFocus(conversationId).catch(() => {});
  };
  const zoomTo = (to: number) => {
    zoomPage(conversationId, to).catch(failed);
  };
  const print = () => {
    void whenShown().then(() => browserPrint(conversationId)).catch(failed);
  };
  const screenshot = () => {
    void whenShown()
      .then(() => browserSnapshot(conversationId))
      .then(() => toast("Screenshot saved to clipboard"), failed);
  };

  /** Does what `key` asks of this tab; false when it has nothing to do it on. */
  const onBrowserKey = useEffectEvent((key: BrowserKey): boolean => {
    if (key === "address") {
      void focusApp().then(() => {
        field.current?.focus();
        field.current?.select();
      });
      return true;
    }
    if (!embedded || !made) return false;
    switch (key) {
      case "back":
      case "forward":
        // The page has the keyboard: the menu took ⌘← or ⌘→ before it, so a field of the page
        // keeps them for its caret.
        act(document.hasFocus() ? key : `${key}-unless-typing`);
        return true;
      case "reload":
        act("reload");
        return true;
      case "force-reload":
        act(mac ? "force-reload" : "reload");
        return true;
      default:
        break;
    }
    if (!native) return false;
    const find = useBrowserFind.getState().tabs[conversationId] ?? NO_FIND;
    switch (key) {
      case "find":
        showFind();
        return true;
      case "find-next":
      case "find-previous":
        if (!find.open || !find.query) showFind();
        else void findInPage(conversationId, find.query, key === "find-next" ? "next" : "previous");
        return true;
      case "zoom-in":
        zoomTo(stepZoom(zoom, 1));
        return true;
      case "zoom-out":
        zoomTo(stepZoom(zoom, -1));
        return true;
      case "zoom-reset":
        zoomTo(1);
        return true;
    }
  });

  // The tab's keys while it is in front (see `browserKey`): ⌘L the address, ⌘←/⌘→, ⌘R and ⇧⌘R
  // the page, ⌘F/⌘G/⇧⌘G finding, ⌘=/⌘-/⌘0 zoom. A field's own ⌘← and ⌘→ stay its own, and a
  // terminal keeps its keys.
  useEffect(() => {
    if (!active) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented) return;
      const key = browserKey(event, mac);
      if (!key) return;
      const target = event.target instanceof Element ? event.target : null;
      if (target?.closest('[data-slot="terminal-pane"]')) return;
      if ((key === "back" || key === "forward") && target?.closest('input, textarea, [contenteditable]:not([contenteditable="false"])')) return;
      if (onBrowserKey(key)) event.preventDefault();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [mac, active]);

  // A page loaded while the find bar is open: its matches are looked for again.
  const wasLoading = useRef(loading);
  useEffect(() => {
    const was = wasLoading.current;
    wasLoading.current = loading;
    if (!was || loading) return;
    const { open, query } = useBrowserFind.getState().tabs[conversationId] ?? NO_FIND;
    if (open && query && native) void findInPage(conversationId, query, "type");
  }, [loading, conversationId, native]);

  const go = (text: string) => {
    const to = browserAddress(text);
    if (!to) return;
    userUsed(conversationId);
    setTyped(null);
    field.current?.blur();
    if (!embedded) {
      openUrl(to).catch(failed);
      return;
    }
    if (onNavigate) { onNavigate(to); return; }
    if (!area.current) return;
    openPage(conversationId, to, pageBounds(area.current), agent).catch(failed);
  };
  const editing = focused || typed !== null;
  const suggestions = useAddressSuggestions(conversationId, focused ? typed : null);
  const suggestionsOpen = focused && suggesting && typed !== null && suggestions.rows.length > 0;
  const pick = (row: Suggestion) => {
    setHighlight(-1);
    if (row.kind === "tab") {
      setTyped(null);
      field.current?.blur();
      const session = sessionWith(row.id);
      if (session) selectTab(session, row.id);
      return;
    }
    go(row.kind === "go" ? row.text : row.url);
  };
  // What Open in external browser opens: what is being typed, else the page.
  const outside = (typed !== null ? browserAddress(typed) : null) ?? (url || null);

  return (
    <div data-pane="browser" data-active={active || undefined} className="@container/new-tab flex min-h-0 flex-1 flex-col">
      <div data-slot="browser-toolbar" className="@container/browser-toolbar border-border flex h-12 shrink-0 items-center gap-1.5 border-b p-2">
        <NavigationCapsule disabled={!embedded || !made} loading={loading} onAct={act} />
        <form
          className="flex min-w-0 flex-1 justify-center px-1 @max-sm/browser-toolbar:px-0"
          onSubmit={(event) => {
            event.preventDefault();
            go(typed ?? url);
          }}
        >
          <div
            ref={address}
            data-slot="browser-address"
            title={page?.title || undefined}
            className="group/address bg-muted/60 ring-border flex h-control-md w-full max-w-browser-address min-w-0 items-center rounded-full p-0.5 ring-1 ring-inset"
          >
            {!editing && <SiteInfo url={url} />}
            <input
              ref={field}
              // ⌘T opens the tab to type an address at once.
              // oxlint-disable-next-line jsx-a11y/no-autofocus
              autoFocus={active && !made}
              value={typed ?? (focused ? url : shownAddress(url, fullUrl))}
              placeholder="Search or enter a URL"
              aria-label="Address"
              role="combobox"
              aria-autocomplete="list"
              aria-expanded={suggestionsOpen}
              aria-controls={suggestionsOpen ? "browser-suggestions" : undefined}
              aria-activedescendant={suggestionsOpen && highlight >= 0 ? `browser-suggestion-${highlight}` : undefined}
              spellCheck={false}
              autoCapitalize="off"
              autoCorrect="off"
              onChange={(event) => {
                setTyped(event.target.value);
                setHighlight(-1);
                setSuggesting(true);
              }}
              onFocus={(event) => {
                setFocused(true);
                // The whole URL replaces the site first, then is selected.
                const input = event.target;
                requestAnimationFrame(() => input.select());
              }}
              onBlur={() => {
                setFocused(false);
                setHighlight(-1);
              }}
              onKeyDown={(event) => {
                if (suggestionsOpen && (event.key === "ArrowDown" || event.key === "ArrowUp")) {
                  event.preventDefault();
                  setHighlight((at) => stepSuggestion(at, event.key === "ArrowDown" ? 1 : -1, suggestions.rows.length));
                  return;
                }
                const row = suggestionsOpen && highlight >= 0 ? suggestions.rows[highlight] : undefined;
                if (event.key === "Enter" && row) {
                  event.preventDefault();
                  pick(row);
                  return;
                }
                if (event.key === "Escape" && suggestionsOpen) {
                  // The first Esc closes the suggestions; the next puts the page's address back.
                  event.stopPropagation();
                  setSuggesting(false);
                  setHighlight(-1);
                  return;
                }
                if (event.key === "Escape" && typed !== null) {
                  event.stopPropagation();
                  setTyped(null);
                }
              }}
              className={cn(
                "browser-address-field text-browser placeholder:text-muted-foreground h-full min-w-0 flex-1 bg-transparent px-2 outline-none",
                // Idle, the site sits in the middle; focused, the address reads from its start.
                focused ? "text-start" : "text-center",
              )}
            />
            {outside && (url || typed !== null) && (
              <TooltipIconButton
                type="button"
                tooltip="Open in external browser"
                size="icon-md"
                className="shrink-0 rounded-full @max-lg/browser-toolbar:hidden"
                // Typing goes on after the click.
                onMouseDown={(event) => event.preventDefault()}
                onClick={() => openOutside(outside)}
              >
                <ArrowUpRight />
              </TooltipIconButton>
            )}
          </div>
          <AddressSuggestions
            anchor={address}
            field={field}
            rows={suggestions.rows}
            open={suggestionsOpen}
            highlight={highlight}
            onPick={pick}
            onHighlight={setHighlight}
            onRemove={suggestions.remove}
            onClose={() => setSuggesting(false)}
          />
          {/* Only over its own tab: a question waits, unanswered, while another tab is in front. */}
          {embedded && <PermissionPrompt id={conversationId} asking={active ? page?.asking : null} anchor={address} />}
        </form>
        {embedded && <DownloadsButton />}
        {mac && (
          <TooltipIconButton
            tooltip="Take a screenshot"
            size="icon-lg"
            className="shrink-0 rounded-full @max-md/browser-toolbar:hidden"
            disabled={!native}
            onClick={screenshot}
          >
            <CameraPhoto />
          </TooltipIconButton>
        )}
        <BrowserOptions
          native={native}
          zoom={zoom}
          onFind={showFind}
          onPrint={print}
          onZoom={zoomTo}
        />
        <TabChatToolbarButton />
      </div>
      {finding && native && <FindBar id={conversationId} input={findInput} onClose={hideFind} />}
      {paused && driver !== null && (
        <Banner
          data-slot="browser-paused"
          text={<>You're using this page, so <span className="font-medium">{driver}</span> is waiting.</>}
        >
          <Button size="sm" onClick={() => handBack(conversationId)}>
            Continue
          </Button>
        </Banner>
      )}
      {page?.blocked && (
        <Banner
          text={<>This can't open here: <span className="text-muted-foreground">{page.blocked}</span></>}
          onDismiss={() => dismissBlocked(conversationId)}
        >
          {webAddress(page.blocked) && (
            <Button size="sm" variant="ghost" onClick={() => page.blocked && openOutside(page.blocked)}>
              Open in browser
            </Button>
          )}
        </Banner>
      )}
      <div
        ref={area}
        data-slot="browser-page"
        className="flex min-h-0 w-full flex-1"
      >
        {!made && (children ?? (
          <div className="text-muted-foreground m-auto p-6 text-center text-sm">
            <Globe aria-hidden className="mx-auto mb-3 size-6" />
            <p className="text-foreground font-medium">Start browsing</p>
            <p className="mt-1">{embedded ? "Enter a URL to open a page" : "Web pages you enter open in your browser"}</p>
          </div>
        ))}
      </div>
    </div>
  );
}
