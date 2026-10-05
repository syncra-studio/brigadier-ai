import {
  ArrowLeft,
  ArrowRight,
  CollapseLg,
  ExpandLg,
  ExternalLink,
  DotsHorizontal,
  Globe,
  Reload,
  Plus,
  X,
} from "@openai/apps-sdk-ui/components/Icon";
import { useCallback, useContext, useEffect, useRef, useState } from "react";

import {
  PanelButtonsRoom,
  SidePanelContext,
} from "@/app/conversation/SidePanel";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { TitlebarButton, TitlebarTips } from "@/components/titlebar-button";
import { TooltipIconButton } from "@/components/assistant-ui/tooltip-icon-button";
import { browserGo, browserPlace, openUrl } from "@/ipc/client";
import type { BrowserBounds } from "@/ipc/generated";
import {
  closeBrowserTab,
  dismissBlocked,
  newBrowserTab,
  openPage,
  selectBrowserTab,
  useBrowsers,
  useBrowserTabs,
} from "@/state/browsers";
import { useApp } from "@/state/store";
import { cn } from "@/lib/utils";
import { toast } from "@/state/toasts";

/**
 * The Browser tab (⌘T): web pages beside the conversation, such as the app a worker is
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

function failed(cause: unknown) {
  toast(cause instanceof Error ? cause.message : String(cause), {
    tone: "error",
  });
}

function openOutside(url: string) {
  openUrl(url).catch(failed);
}

/** Only this pane has tabs. Each page owns a distinct native webview. */
export function BrowserTab({ conversationId }: { conversationId: string }) {
  const tabs = useBrowserTabs((s) => s.conversations[conversationId]);
  const pages = useBrowsers((s) => s.pages);
  const { hide, state, setFullscreen } = useContext(SidePanelContext);
  const host = useRef<HTMLDivElement>(null);
  const mac = useApp((s) => s.info?.platform === "macos");
  const create = useCallback(
    () => newBrowserTab(conversationId),
    [conversationId],
  );
  useEffect(() => {
    if (!useBrowserTabs.getState().conversations[conversationId]?.ids.length)
      create();
  }, [conversationId, create]);
  useEffect(() => {
    const key = (event: KeyboardEvent) => {
      if (
        !(event.target instanceof Element) ||
        !event.target.closest('[data-pane="browser"]')
      )
        return;
      const command = mac ? event.metaKey : event.ctrlKey;
      if (!command || event.altKey) return;
      if (
        event.shiftKey &&
        ["BracketLeft", "BracketRight"].includes(event.code) &&
        tabs?.ids.length
      ) {
        event.preventDefault();
        const index = tabs.ids.indexOf(tabs.active);
        const next =
          (index + (event.code === "BracketLeft" ? -1 : 1) + tabs.ids.length) %
          tabs.ids.length;
        selectBrowserTab(conversationId, tabs.ids[next]!);
        return;
      }
      if (event.shiftKey) return;
      if (event.code === "KeyT") {
        event.preventDefault();
        create();
      }
      if (event.code === "KeyW" && tabs?.active) {
        event.preventDefault();
        closeBrowserTab(conversationId, tabs.active);
        if (tabs.ids.length === 1) hide();
      }
    };
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  }, [conversationId, create, hide, mac, tabs]);
  useEffect(() => {
    const selected = host.current?.querySelector<HTMLElement>(
      `[data-page="${tabs?.active}"]`,
    );
    if (!selected) return;
    const strip = host.current!;
    if (selected.offsetLeft < strip.scrollLeft)
      strip.scrollLeft = selected.offsetLeft;
    else if (
      selected.offsetLeft + selected.offsetWidth >
      strip.scrollLeft + strip.clientWidth
    )
      strip.scrollLeft =
        selected.offsetLeft + selected.offsetWidth - strip.clientWidth;
  }, [tabs?.active]);
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex h-titlebar shrink-0 items-center gap-1 px-2">
        <div
          ref={host}
          role="tablist"
          aria-label="Browser pages"
          className="hide-scrollbar flex min-w-0 flex-1 overflow-x-auto"
        >
          {tabs?.ids.map((id) => (
            <div
              key={id}
              className={cn(
                "group/tab flex h-8 min-w-16 max-w-60 flex-1 items-center gap-1 rounded-lg px-2 text-sm",
                id === tabs.active
                  ? "bg-panel-tab shadow-panel-tab"
                  : "text-toolbar-foreground hover:bg-toolbar-hover",
              )}
            >
              <button
                role="tab"
                data-page={id}
                type="button"
                aria-selected={id === tabs.active}
                onClick={() => selectBrowserTab(conversationId, id)}
                className="flex min-w-0 flex-1 items-center gap-1.5 outline-none focus-visible:ring-1 focus-visible:ring-ring"
              >
                <Globe className="size-icon-sm shrink-0" />
                <span className="truncate">
                  {pages[id]?.title || pages[id]?.url || "New tab"}
                </span>
              </button>
              <button
                type="button"
                aria-label={`Close ${pages[id]?.title || "browser"} tab`}
                onClick={() => {
                  closeBrowserTab(conversationId, id);
                  if (tabs.ids.length === 1) hide();
                }}
                className="hover:bg-toolbar-hover flex size-5 shrink-0 items-center justify-center rounded opacity-60 hover:opacity-100 focus-visible:ring-1 focus-visible:ring-ring"
              >
                <X className="size-icon-xs" />
              </button>
            </div>
          ))}
        </div>
        <TitlebarTips>
          <TitlebarButton
            tooltip="New browser tab"
            shortcut="⌘T"
            onClick={create}
          >
            <Plus />
          </TitlebarButton>
          <TitlebarButton
            tooltip={state.fullscreen ? "Exit full view" : "Enter full view"}
            shortcut="⌘⇧F"
            aria-pressed={state.fullscreen}
            onClick={() => setFullscreen(!state.fullscreen)}
          >
            {state.fullscreen ? <CollapseLg /> : <ExpandLg />}
          </TitlebarButton>
          <TitlebarButton tooltip="Close Browser" onClick={hide}>
            <X />
          </TitlebarButton>
        </TitlebarTips>
        <PanelButtonsRoom />
      </div>
      {tabs?.active && (
        <BrowserPageView
          key={tabs.active}
          conversationId={tabs.active}
          initialUrl={tabs.restoredUrls?.[tabs.active]}
        />
      )}
    </div>
  );
}

function BrowserPageView({
  conversationId,
  initialUrl,
}: {
  conversationId: string;
  initialUrl?: string | undefined;
}) {
  const page = useBrowsers((s) => s.pages[conversationId]);
  const embedded = useApp((s) => s.info?.platform !== "linux");
  const mac = useApp((s) => s.info?.platform === "macos");
  const area = useRef<HTMLDivElement>(null);
  const field = useRef<HTMLInputElement>(null);
  const [typed, setTyped] = useState<string | null>(null);
  const made = page !== undefined;

  useEffect(() => {
    if (!initialUrl || made || !area.current) return;
    const rect = area.current.getBoundingClientRect();
    void openPage(conversationId, initialUrl, {
      x: rect.left,
      y: rect.top,
      width: rect.width,
      height: rect.height,
    }).catch(failed);
  }, [conversationId, initialUrl, made]);

  // Keep the page over this tab's area, and hide it when the tab goes or is covered.
  useEffect(() => {
    const element = area.current;
    if (!made || !element) return;
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
    const clip = element.closest("[data-slot=side-panel-clip]");
    const place = () => {
      const rect = element.getBoundingClientRect();
      const box = clip?.getBoundingClientRect();
      const whole =
        !box || (rect.left >= box.left - 0.5 && rect.right <= box.right + 0.5);
      const bounds: BrowserBounds | null =
        whole && rect.width > 0 && rect.height > 0 && !covered(rect, layers)
          ? {
              x: rect.left,
              y: rect.top,
              width: rect.width,
              height: rect.height,
            }
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
  }, [conversationId, made]);

  // ⌘L (Ctrl+L) puts the cursor in the address field, as in a browser.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const command = mac ? event.metaKey : event.ctrlKey;
      if (!command || event.shiftKey || event.altKey || event.code !== "KeyL")
        return;
      event.preventDefault();
      field.current?.focus();
      field.current?.select();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [mac]);

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
    const rect = area.current?.getBoundingClientRect();
    if (!rect) return;
    openPage(conversationId, url, {
      x: rect.left,
      y: rect.top,
      width: rect.width,
      height: rect.height,
    }).catch(failed);
  };
  const act = (action: "back" | "forward" | "reload" | "stop") => {
    browserGo(conversationId, action).catch(() => {});
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="border-border flex shrink-0 items-center gap-1 border-b p-2">
        {embedded && (
          <div className="bg-muted/50 flex shrink-0 items-center gap-0.5 rounded-full px-1">
            <TooltipIconButton
              tooltip="Back"
              disabled={!made}
              onClick={() => act("back")}
            >
              <ArrowLeft />
            </TooltipIconButton>
            <TooltipIconButton
              tooltip="Forward"
              disabled={!made}
              onClick={() => act("forward")}
            >
              <ArrowRight />
            </TooltipIconButton>
            {page?.loading ? (
              <TooltipIconButton tooltip="Stop" onClick={() => act("stop")}>
                <X />
              </TooltipIconButton>
            ) : (
              <TooltipIconButton
                tooltip="Reload"
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
            className="bg-muted/50 flex h-control-md items-center gap-1.5 rounded-full px-3"
          >
            <Globe className="text-muted-foreground size-icon-sm shrink-0" />
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
              className="placeholder:text-muted-foreground min-w-0 flex-1 bg-transparent text-sm outline-none"
            />
          </label>
        </form>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <TooltipIconButton tooltip="Browser options">
              <DotsHorizontal />
            </TooltipIconButton>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
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
      <div ref={area} data-slot="browser-page" className="flex min-h-0 flex-1">
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
