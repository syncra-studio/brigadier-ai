import {
  ArrowLeft,
  ArrowRight,
  ArrowUpRight,
  DotsHorizontal,
  Globe,
  Info,
  Lock,
  Minus,
  Plus,
  Reload,
  Warning,
  X,
} from "@openai/apps-sdk-ui/components/Icon";
import { type ReactNode, useEffect, useRef, useState } from "react";

import { browserAddress, shownAddress, siteSecurity, webAddress } from "./browserAddress";

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
import { browserGo, browserPlace, openUrl } from "@/ipc/client";
import type { BrowserBounds } from "@/ipc/generated";
import {
  dismissBlocked,
  openPage,
  useBrowsers,
} from "@/state/browsers";
import { openSettings } from "@/state/actions";
import { openBrowserSettings } from "@/state/browserSettings";
import { useBrowserOn } from "@/state/links";
import { useApp } from "@/state/store";
import { cn } from "@/lib/utils";
import { toast } from "@/state/toasts";

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

/** A ⋯ menu row the shell's page does natively; D2 brings them to life. */
const NATIVE_ITEM = "data-[disabled]:opacity-40";

/**
 * The ⋯ menu: the page's own actions, then where the browser's data lives in Settings.
 * Find in page, Print, Zoom and the screenshot wait for the page to do them natively
 * (`data-native`), so they show disabled for now.
 */
function BrowserOptions() {
  const mac = useApp((s) => s.info?.platform === "macos");
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
      >
        <DropdownMenuItem disabled data-native="find" className={NATIVE_ITEM}>Find in page</DropdownMenuItem>
        <DropdownMenuItem disabled data-native="print" className={NATIVE_ITEM}>Print</DropdownMenuItem>
        <DropdownMenuSeparator className="mx-2" />
        <div role="group" aria-label="Zoom" data-native="zoom" className="text-browser flex h-browser-menu-item items-center gap-1 ps-2 pe-1">
          <span className="flex-1">Zoom</span>
          <div className="border-border bg-foreground/5 flex h-browser-zoom items-center rounded-lg border text-xs">
            <button type="button" disabled aria-label="Zoom out" className="flex size-6 items-center justify-center disabled:opacity-40">
              <Minus className="size-icon-sm" />
            </button>
            <span className="border-border w-browser-zoom-value border-x text-center tabular-nums">100%</span>
            <button type="button" disabled aria-label="Zoom in" className="flex size-6 items-center justify-center disabled:opacity-40">
              <Plus className="size-icon-sm" />
            </button>
          </div>
          <button type="button" disabled aria-label="Reset zoom" className="flex size-6 items-center justify-center rounded-full disabled:opacity-40">
            <Reload className="size-icon-sm" />
          </button>
        </div>
        <DropdownMenuSeparator className="mx-2" />
        <DropdownMenuItem disabled data-native="screenshot" className={NATIVE_ITEM}>Take a screenshot</DropdownMenuItem>
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
  const area = useRef<HTMLDivElement>(null);
  const field = useRef<HTMLInputElement>(null);
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
    let frame = 0;
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
      const bounds: BrowserBounds | null =
        !element.closest("[inert]") &&
        placedBounds.width > 0 &&
        placedBounds.height > 0 &&
        !covered(rendered, layers)
          ? placedBounds
          : null;
      const key = JSON.stringify(bounds);
      if (key !== placed) {
        placed = key;
        browserPlace(conversationId, bounds).catch(() => {});
      }
      frame = requestAnimationFrame(place);
    };
    frame = requestAnimationFrame(place);
    return () => {
      cancelAnimationFrame(frame);
      observer.disconnect();
      browserPlace(conversationId, null).catch(() => {});
    };
  }, [conversationId, page?.ready, active]);

  // ⌘L (Ctrl+L) puts the cursor in the address field, as in a browser.
  useEffect(() => {
    if (!active) return;
    const onKeyDown = (event: KeyboardEvent) => {
      const command = mac ? event.metaKey : event.ctrlKey;
      if (!command || event.shiftKey || event.altKey) return;
      if (event.code !== "KeyL") return;
      event.preventDefault();
      field.current?.focus();
      field.current?.select();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [mac, active]);

  const go = (text: string) => {
    const url = browserAddress(text);
    if (!url) return;
    setTyped(null);
    field.current?.blur();
    if (!embedded) {
      openUrl(url).catch(failed);
      return;
    }
    if (onNavigate) { onNavigate(url); return; }
    if (!area.current) return;
    openPage(conversationId, url, pageBounds(area.current), agent).catch(failed);
  };
  const act = (action: Parameters<typeof browserGo>[1]) => {
    browserGo(conversationId, action).catch(failed);
  };

  const url = page?.url ?? "";
  const editing = focused || typed !== null;
  // What Open in external browser opens: what is being typed, else the page.
  const outside = (typed !== null ? browserAddress(typed) : null) ?? (url || null);
  const loading = page?.loading === true;

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
        <BrowserOptions />
      </div>
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
