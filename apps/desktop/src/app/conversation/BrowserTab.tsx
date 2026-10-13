import {
  ArrowLeft,
  ArrowRight,
  ArrowUpRight,
  ChevronDown,
  ChevronUp,
  DotsHorizontal,
  Globe,
  Info,
  Lock,
  Minus,
  Plus,
  Reload,
  Search,
  Warning,
  X,
} from "@openai/apps-sdk-ui/components/Icon";
import { type ReactNode, type RefObject, useEffect, useEffectEvent, useRef, useState } from "react";

import { browserAddress, shownAddress, siteSecurity, webAddress } from "./browserAddress";
import { showPage } from "./browserLayer";
import { type BrowserKey, browserKey, stepZoom, ZOOM_LEAST, ZOOM_MOST, zoomLabel } from "./browserKeys";

import { Spinner } from "@/components/glyphs/spinner";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { TooltipIconButton } from "@/components/assistant-ui/tooltip-icon-button";
import { browserFocus, browserGo, browserPlace, browserPrint, browserSnapshot, openUrl } from "@/ipc/client";
import type { BrowserBounds } from "@/ipc/generated";
import { closeFind, findInPage, findLabel, NO_FIND, openFind, useBrowserFind } from "@/state/browserFind";
import {
  dismissBlocked,
  openPage,
  useBrowsers,
  zoomPage,
} from "@/state/browsers";
import { openSettings } from "@/state/actions";
import { openBrowserSettings } from "@/state/browserSettings";
import { useBrowserOn } from "@/state/links";
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

/** Back, Forward and Reload: round, and faint while they can't act. */
const NAV_BUTTON = "rounded-full disabled:opacity-40 disabled:text-muted-foreground";

/** What the site information popover says of each kind of page. */
const SECURITY = {
  secure: {
    label: "View site information",
    Icon: Lock,
    title: "Connection is secure",
    detail: "Passwords, card numbers and anything else you send this site stay private on the way there.",
  },
  insecure: {
    label: "Not secure, view site information",
    Icon: Warning,
    title: "Connection is not secure",
    detail: "Don't enter passwords, card numbers or other private information here: others on the network could read them.",
  },
  local: {
    label: "View site information",
    Icon: Info,
    title: "Connection is not secure",
    detail: "This page comes from your own computer, so what you send it doesn't travel over the network.",
  },
} as const;

/**
 * The address field's start: a lock for https (shown while the pointer is over the field), "Not
 * secure" for http, nothing on the New tab page. It opens what is known of the connection.
 */
function SiteInfo({ url }: { url: string }) {
  const security = siteSecurity(url);
  const mac = useApp((s) => s.info?.platform === "macos");
  const [open, setOpen] = useState(false);
  if (!security) return null;
  const { label, Icon, title, detail } = SECURITY[security];
  let host = url;
  try {
    host = new URL(url).host;
  } catch {
    // siteSecurity read it already.
  }
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <TooltipIconButton
          tooltip={label}
          size="icon-md"
          data-slot="browser-site-info"
          data-security={security}
          className={cn(
            "shrink-0 rounded-full",
            security === "insecure"
              ? "text-destructive w-auto gap-1 px-2 text-xs"
              : "opacity-0 group-hover/address:opacity-100 focus-visible:opacity-100 data-[state=open]:opacity-100",
          )}
        >
          <Icon />
          {security === "insecure" && <span aria-hidden>Not secure</span>}
        </TooltipIconButton>
      </PopoverTrigger>
      <PopoverContent align="start" aria-label="Site information" className="w-browser-site-info p-3">
        <p className="text-browser truncate font-medium">{host}</p>
        <div className="mt-3 flex items-start gap-2">
          <Icon aria-hidden className={cn("size-icon-md mt-0.5 shrink-0", security === "insecure" && "text-destructive")} />
          <div className="min-w-0">
            <p className="text-browser">{title}</p>
            <p className="text-muted-foreground mt-0.5 text-xs">{detail}</p>
          </div>
        </div>
        {mac && (
          <>
            <div className="bg-border -mx-3 my-3 h-px" />
            <button
              type="button"
              onClick={() => {
                setOpen(false);
                openBrowserSettings("sites");
              }}
              className="hover:bg-foreground/8 text-browser h-browser-menu-item -mx-1 flex w-full items-center rounded-capsule px-1"
            >
              Site settings
            </button>
          </>
        )}
      </PopoverContent>
    </Popover>
  );
}

/** The ⋯ menu's rows the page does natively (macOS), faint while there is no page to act on. */
const NATIVE_ITEM = "data-[disabled]:opacity-40";

/**
 * The ⋯ menu: the page's own actions (Find in page, Print, Zoom and a screenshot, which the
 * page does natively on macOS and only once there is one), then where the browser's data lives
 * in Settings.
 */
function BrowserOptions({
  native,
  zoom,
  onFind,
  onPrint,
  onZoom,
  onScreenshot,
}: {
  /** A page is open and does them natively. */
  native: boolean;
  zoom: number;
  onFind: () => void;
  onPrint: () => void;
  onZoom: (zoom: number) => void;
  onScreenshot: () => void;
}) {
  const mac = useApp((s) => s.info?.platform === "macos");
  // Find in page puts the keyboard in its field: the menu doesn't take it back to its button.
  const keepFocus = useRef(false);
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <TooltipIconButton tooltip="Browser options" size="icon-lg" className="shrink-0 rounded-full">
          <DotsHorizontal />
        </TooltipIconButton>
      </DropdownMenuTrigger>
      <DropdownMenuContent
        align="end"
        data-slot="browser-options"
        className="w-browser-menu rounded-browser-menu [&_[role=menuitem]]:h-browser-menu-item [&_[role=menuitem]]:text-browser"
        onCloseAutoFocus={(event) => {
          if (!keepFocus.current) return;
          keepFocus.current = false;
          event.preventDefault();
        }}
      >
        <DropdownMenuItem
          disabled={!native}
          className={NATIVE_ITEM}
          onSelect={() => {
            keepFocus.current = true;
            onFind();
          }}
        >
          Find in page
        </DropdownMenuItem>
        <DropdownMenuItem disabled={!native} className={NATIVE_ITEM} onSelect={onPrint}>Print</DropdownMenuItem>
        <DropdownMenuSeparator className="mx-2" />
        <div role="group" aria-label="Zoom" className="text-browser flex h-browser-menu-item items-center gap-1 ps-2 pe-1">
          <span className="flex-1">Zoom</span>
          <div className="border-border bg-foreground/5 flex h-browser-zoom items-center rounded-lg border text-xs">
            <button
              type="button"
              disabled={!native || zoom <= ZOOM_LEAST}
              aria-label="Zoom out"
              onClick={() => onZoom(stepZoom(zoom, -1))}
              className="hover:bg-foreground/8 flex size-6 items-center justify-center rounded-s-lg disabled:opacity-40 disabled:hover:bg-transparent"
            >
              <Minus className="size-icon-sm" />
            </button>
            <span data-slot="browser-zoom" aria-live="polite" className="border-border w-browser-zoom-value border-x text-center tabular-nums">
              {zoomLabel(zoom)}
            </span>
            <button
              type="button"
              disabled={!native || zoom >= ZOOM_MOST}
              aria-label="Zoom in"
              onClick={() => onZoom(stepZoom(zoom, 1))}
              className="hover:bg-foreground/8 flex size-6 items-center justify-center rounded-e-lg disabled:opacity-40 disabled:hover:bg-transparent"
            >
              <Plus className="size-icon-sm" />
            </button>
          </div>
          <button
            type="button"
            disabled={!native || zoom === 1}
            aria-label="Reset zoom"
            onClick={() => onZoom(1)}
            className="hover:bg-foreground/8 flex size-6 items-center justify-center rounded-full disabled:opacity-40 disabled:hover:bg-transparent"
          >
            <Reload className="size-icon-sm" />
          </button>
        </div>
        <DropdownMenuSeparator className="mx-2" />
        <DropdownMenuItem disabled={!native} className={NATIVE_ITEM} onSelect={onScreenshot}>Take a screenshot</DropdownMenuItem>
        <DropdownMenuSeparator className="mx-2" />
        {mac && <DropdownMenuItem onSelect={() => openSettings("browserImport")}>Import Browser Data…</DropdownMenuItem>}
        <DropdownMenuItem onSelect={() => openBrowserSettings("downloads")}>Downloads</DropdownMenuItem>
        <DropdownMenuItem onSelect={() => openBrowserSettings("history")}>History</DropdownMenuItem>
        <DropdownMenuItem onSelect={() => openBrowserSettings("clear")}>Clear browsing data</DropdownMenuItem>
        <DropdownMenuSeparator className="mx-2" />
        <DropdownMenuItem onSelect={() => openBrowserSettings()}>Browser settings</DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/**
 * The page's find bar, under the toolbar at its end (the page, drawn over the window, would hide
 * it anywhere over the page): the text, which match of how many, the previous and next match,
 * and close. Enter goes to the next match, Shift+Enter the previous, Esc closes.
 */
function FindBar({ id, input, onClose }: { id: string; input: RefObject<HTMLInputElement | null>; onClose: () => void }) {
  const find = useBrowserFind((s) => s.tabs[id]) ?? NO_FIND;
  const label = findLabel(find);
  const step = (backwards: boolean) => {
    if (find.query) void findInPage(id, find.query, backwards ? "previous" : "next");
  };
  return (
    <div data-slot="browser-find-row" className="border-border flex shrink-0 justify-end border-b px-2 py-1">
      <div
        role="search"
        aria-label="Find in page"
        data-slot="browser-find"
        className="bg-popover text-popover-foreground shadow-browser-find rounded-menu flex h-browser-find-row w-browser-find max-w-full min-w-0 items-center gap-2 ps-4 pe-2"
      >
        <Search aria-hidden className="text-muted-foreground size-icon-md shrink-0" />
        <input
          ref={input}
          value={find.query}
          placeholder="Find in page…"
          aria-label="Find in page"
          spellCheck={false}
          autoCapitalize="off"
          autoCorrect="off"
          onChange={(event) => void findInPage(id, event.target.value, "type")}
          onFocus={(event) => event.target.select()}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault();
              step(event.shiftKey);
            } else if (event.key === "Escape") {
              event.preventDefault();
              event.stopPropagation();
              onClose();
            }
          }}
          className="text-browser placeholder:text-muted-foreground h-6 min-w-0 flex-1 bg-transparent outline-none"
        />
        <span data-slot="browser-find-count" aria-live="polite" className="text-muted-foreground shrink-0 text-xs whitespace-nowrap tabular-nums select-none">
          {label}
        </span>
        <TooltipIconButton tooltip="Previous match" shortcut="⇧↵" size="icon-xs" disabled={!find.found} onClick={() => step(true)}>
          <ChevronUp />
        </TooltipIconButton>
        <TooltipIconButton tooltip="Next match" shortcut="↵" size="icon-xs" disabled={!find.found} onClick={() => step(false)}>
          <ChevronDown />
        </TooltipIconButton>
        <span aria-hidden className="bg-border h-4 w-px shrink-0" />
        <TooltipIconButton tooltip="Close find" size="icon-xs" onClick={onClose}>
          <X />
        </TooltipIconButton>
      </div>
    </div>
  );
}

const frame = () => new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));

/** Gives the keyboard back to the app's own page, which a tab's native page may hold. */
async function focusApp(): Promise<void> {
  if (!document.hasFocus()) await browserFocus(null).catch(() => {});
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
      next = requestAnimationFrame(place);
    };
    next = requestAnimationFrame(place);
    return () => {
      cancelAnimationFrame(next);
      observer.disconnect();
      shown.current = false;
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
  // What Open in external browser opens: what is being typed, else the page.
  const outside = (typed !== null ? browserAddress(typed) : null) ?? (url || null);

  return (
    <div data-pane="browser" data-active={active || undefined} className="@container/new-tab flex min-h-0 flex-1 flex-col">
      <div data-slot="browser-toolbar" className="@container/browser-toolbar border-border flex h-12 shrink-0 items-center gap-1.5 border-b p-2">
        <div role="group" aria-label="Navigation" data-slot="browser-navigation" className="bg-muted/60 flex h-control-md shrink-0 items-center rounded-full p-0.5">
          <TooltipIconButton tooltip="Back" size="icon-md" className={NAV_BUTTON} disabled={!embedded || !made} onClick={() => act("back")}>
            <ArrowLeft />
          </TooltipIconButton>
          <TooltipIconButton tooltip="Forward" size="icon-md" className={NAV_BUTTON} disabled={!embedded || !made} onClick={() => act("forward")}>
            <ArrowRight />
          </TooltipIconButton>
          <span aria-hidden className="bg-border mx-1 h-4 w-px shrink-0" />
          {/* While the page loads the spinner stands in for Reload, and stops the load. */}
          <TooltipIconButton
            tooltip={loading ? "Stop loading" : "Reload"}
            size="icon-md"
            className={NAV_BUTTON}
            aria-busy={loading}
            disabled={!embedded || !made}
            onClick={() => act(loading ? "stop" : "reload")}
          >
            {loading ? <Spinner className="animate-spin" /> : <Reload />}
          </TooltipIconButton>
        </div>
        <form
          className="flex min-w-0 flex-1 justify-center px-1 @max-sm/browser-toolbar:px-0"
          onSubmit={(event) => {
            event.preventDefault();
            go(typed ?? url);
          }}
        >
          <div
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
              spellCheck={false}
              autoCapitalize="off"
              autoCorrect="off"
              onChange={(event) => setTyped(event.target.value)}
              onFocus={(event) => {
                setFocused(true);
                // The whole URL replaces the site first, then is selected.
                const input = event.target;
                requestAnimationFrame(() => input.select());
              }}
              onBlur={() => setFocused(false)}
              onKeyDown={(event) => {
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
        </form>
        <BrowserOptions
          native={native}
          zoom={zoom}
          onFind={showFind}
          onPrint={print}
          onZoom={zoomTo}
          onScreenshot={screenshot}
        />
        <TabChatToolbarButton />
      </div>
      {finding && native && <FindBar id={conversationId} input={findInput} onClose={hideFind} />}
      {page?.blocked && (
        <div
          role="status"
          className="border-border bg-muted/40 flex shrink-0 items-center gap-2 border-b px-3 py-1.5 text-sm"
        >
          <span className="min-w-0 flex-1 truncate">
            This can't open here:{" "}
            <span className="text-muted-foreground">{page.blocked}</span>
          </span>
          {webAddress(page.blocked) && (
            <button
              type="button"
              onClick={() => page.blocked && openOutside(page.blocked)}
              className="hover:bg-foreground/5 rounded-control h-control-sm shrink-0 px-2 transition-colors"
            >
              Open in browser
            </button>
          )}
          <TooltipIconButton
            tooltip="Dismiss"
            size="icon-xs"
            onClick={() => dismissBlocked(conversationId)}
          >
            <X />
          </TooltipIconButton>
        </div>
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
