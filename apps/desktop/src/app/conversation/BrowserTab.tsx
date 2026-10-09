import {
  ArrowLeft,
  ArrowRight,
  ExternalLink,
  DotsHorizontal,
  Globe,
  Reload,
  X,
} from "@openai/apps-sdk-ui/components/Icon";
import { useEffect, useRef, useState } from "react";

import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { TooltipIconButton } from "@/components/assistant-ui/tooltip-icon-button";
import { browserGo, browserPlace, openUrl } from "@/ipc/client";
import type { BrowserBounds } from "@/ipc/generated";
import {
  dismissBlocked,
  openPage,
  useBrowsers,
} from "@/state/browsers";
import { useApp } from "@/state/store";
import { cn } from "@/lib/utils";
import { toast } from "@/state/toasts";

/**
 * A browser main tab (⌘⇧B): one page, such as the app a worker is
 * running on localhost. The page is a system webview the shell lays over this tab's area (see
 * `src-tauri/src/browser.rs`); it is made on the first address entered and hidden whenever the
 * tab is, or something of the app's (a menu, a dialog) opens over it.
 */

/** What the address field means as a web address, or null. Local servers get http. */
export function webAddress(typed: string): string | null {
  const text = typed.trim();
  if (!text || /\s/.test(text)) return null;
  const scheme = /^([a-z][a-z0-9+.-]*):\/\//i.exec(text)?.[1]?.toLowerCase();
  if (scheme) return scheme === "http" || scheme === "https" ? text : null;
  const host = text.split(/[/?#]/)[0] ?? "";
  if (
    /^(localhost|127\.\d+\.\d+\.\d+|0\.0\.0\.0|\[::1\])(:\d+)?$/i.test(host)
  ) {
    return `http://${text}`;
  }
  return host.includes(".") ? `https://${text}` : null;
}

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

function siteAddress(url: string | undefined): URL | null {
  try {
    const site = new URL(url ?? "");
    return ["http:", "https:"].includes(site.protocol) ? site : null;
  } catch {
    return null;
  }
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

export function BrowserTab({
  conversationId,
  initialUrl,
  active,
}: {
  conversationId: string;
  initialUrl?: string | undefined;
  active: boolean;
}) {
  const page = useBrowsers((s) => s.pages[conversationId]);
  const embedded = useApp((s) => s.info?.platform !== "linux");
  const mac = useApp((s) => s.info?.platform === "macos");
  const area = useRef<HTMLDivElement>(null);
  const field = useRef<HTMLInputElement>(null);
  const [typed, setTyped] = useState<string | null>(null);
  const made = page !== undefined;
  const site = siteAddress(page?.url);
  useEffect(() => {
    if (!active || !initialUrl || made || !area.current) return;
    void openPage(conversationId, initialUrl, pageBounds(area.current)).catch(
      failed,
    );
  }, [conversationId, initialUrl, made, active]);

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
    const url = webAddress(text);
    if (!url) {
      toast("Enter a web address, like localhost:3000 or example.com", {
        tone: "error",
      });
      return;
    }
    setTyped(null);
    field.current?.blur();
    if (!embedded) {
      openUrl(url).catch(failed);
      return;
    }
    if (!area.current) return;
    openPage(conversationId, url, pageBounds(area.current)).catch(failed);
  };
  const act = (action: Parameters<typeof browserGo>[1]) => {
    browserGo(conversationId, action).catch(failed);
  };

  return (
    <div data-pane="browser" data-active={active || undefined} className="flex min-h-0 flex-1 flex-col">
      <div className="border-border flex shrink-0 items-center gap-1 border-b p-2">
        {embedded && (
          <div className="flex shrink-0 items-center gap-0.5">
            <TooltipIconButton
              tooltip="Back"
              className="size-7 rounded-full"
              disabled={!made}
              onClick={() => act("back")}
            >
              <ArrowLeft />
            </TooltipIconButton>
            <span aria-hidden className="bg-border mx-0.5 h-4 w-px" />
            <TooltipIconButton
              tooltip="Forward"
              className="size-7 rounded-full"
              disabled={!made}
              onClick={() => act("forward")}
            >
              <ArrowRight />
            </TooltipIconButton>
            {page?.loading ? (
              <TooltipIconButton
                tooltip="Stop"
                className="size-7 rounded-full"
                onClick={() => act("stop")}
              >
                <X />
              </TooltipIconButton>
            ) : (
              <TooltipIconButton
                tooltip="Reload"
                className="size-7 rounded-full"
                disabled={!made}
                onClick={() => act("reload")}
              >
                <Reload />
              </TooltipIconButton>
            )}
          </div>
        )}
        <form
          className="min-w-0 flex-1"
          onSubmit={(event) => {
            event.preventDefault();
            go(typed ?? page?.url ?? "");
          }}
        >
          <label
            title={page?.title || undefined}
            className="border-border/60 flex h-8 items-center gap-1.5 rounded-full border px-2"
          >
            {site ? (
              <Popover>
                <PopoverTrigger asChild>
                  <TooltipIconButton
                    type="button"
                    tooltip="View site information"
                    className="size-7 rounded-full"
                  >
                    <Globe />
                  </TooltipIconButton>
                </PopoverTrigger>
                <PopoverContent
                  align="start"
                  className="w-browser-menu text-sm"
                >
                  <p className="truncate font-medium">{site.host}</p>
                  <p className="text-muted-foreground mt-2">
                    This page uses{" "}
                    {site.protocol === "https:" ? "HTTPS" : "HTTP"}.
                  </p>
                </PopoverContent>
              </Popover>
            ) : (
              <Globe className="text-muted-foreground size-icon-sm shrink-0" />
            )}
            <input
              ref={field}
              // ⌘T opens the tab to type an address at once.
              // oxlint-disable-next-line jsx-a11y/no-autofocus
              autoFocus={!made}
              value={typed ?? page?.url ?? ""}
              placeholder="Search or enter a URL"
              aria-label="Address"
              spellCheck={false}
              autoCapitalize="off"
              autoCorrect="off"
              onChange={(event) => setTyped(event.target.value)}
              onFocus={(event) => event.target.select()}
              onKeyDown={(event) => {
                if (event.key === "Escape" && typed !== null) {
                  event.stopPropagation();
                  setTyped(null);
                }
              }}
              className={cn(
                "placeholder:text-muted-foreground min-w-0 flex-1 bg-transparent text-sm outline-none",
                !made && "text-center",
              )}
            />
          </label>
        </form>
        {page?.url && (
          <TooltipIconButton
            tooltip="Open in external browser"
            onClick={() => openOutside(page.url)}
          >
            <ExternalLink />
          </TooltipIconButton>
        )}
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <TooltipIconButton
              tooltip="Browser options"
              className="size-8 rounded-full"
            >
              <DotsHorizontal />
            </TooltipIconButton>
          </DropdownMenuTrigger>
          <DropdownMenuContent
            align="end"
            className="w-browser-menu rounded-browser-menu [&_[role=menuitem]]:h-browser-menu-item"
          >
            <DropdownMenuItem disabled={!made} onSelect={() => act("reload")}>
              <Reload />
              Reload page
            </DropdownMenuItem>
            <DropdownMenuItem
              disabled={!page?.url}
              onSelect={() => {
                if (page?.url)
                  void navigator.clipboard.writeText(page.url).catch(failed);
              }}
            >
              Copy address
            </DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem
              disabled={!page?.url}
              onSelect={() => page?.url && openOutside(page.url)}
            >
              <ExternalLink />
              Open in external browser
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
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
        {!made && (
          <p className="text-muted-foreground m-auto max-w-xs p-4 text-center text-sm">
            {embedded
              ? "Open a web page beside the conversation, such as the app a worker runs on localhost"
              : "Web pages you enter open in your browser"}
          </p>
        )}
      </div>
    </div>
  );
}
