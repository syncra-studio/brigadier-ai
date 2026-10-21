import { useRef } from "react";

import { stepZoom, ZOOM_LEAST, ZOOM_MOST, zoomLabel } from "@/app/conversation/browserKeys";
import { DotsHorizontal, Minus, Plus, Reload } from "@/components/icons";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/overlays";
import { TooltipIconButton } from "@/components/controls";
import { openSettings } from "@/state/actions";
import { openBrowserSettings } from "@/state/browserSettings";
import { useApp } from "@/state/store";

/** The ⋯ menu's rows the page does natively (macOS), faint while there is no page to act on. */
const NATIVE_ITEM = "data-[disabled]:opacity-40";

/**
 * The ⋯ menu: the page's own actions (Find in page, Print and Zoom, which the page does natively
 * on macOS and only once there is one), then where the browser's data lives in Settings.
 */
export function BrowserOptions({
  native,
  zoom,
  onFind,
  onPrint,
  onZoom,
}: {
  /** A page is open and does them natively. */
  native: boolean;
  zoom: number;
  onFind: () => void;
  onPrint: () => void;
  onZoom: (zoom: number) => void;
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
