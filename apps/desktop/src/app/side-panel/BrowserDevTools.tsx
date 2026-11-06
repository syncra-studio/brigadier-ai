import { type ReactNode, useEffect, useLayoutEffect, useRef, useState } from "react";

import { showPage } from "@/app/conversation/browserLayer";
import { followLayout } from "@/app/conversation/pageWatch";
import { typeInto } from "@/app/conversation/composerDraft";
import { Button, TooltipIconButton } from "@/components/controls";
import { Terminal } from "@/components/icons";
import { Banner } from "@/components/shared";
import { DropdownMenu, DropdownMenuCheckboxItem, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/overlays";
import { browserFocus } from "@/ipc/client";
import type { BrowserBounds } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { driverOf, useAgentPause, userUsed } from "@/state/agentPause";
import {
  changeDev,
  closeDevTools,
  devOf,
  elementText,
  fitScale,
  hasDevTools,
  openDevTools,
  placeDevTools,
  startPicking,
  stopPicking,
  useBrowserDev,
} from "@/state/browserDev";
import { useBrowsers } from "@/state/browsers";
import { sessionWith } from "@/state/sessionTabs";
import { toast } from "@/state/toasts";

import { toggleDevice, useDeviceSize } from "./BrowserDeviceSizes";

function failed(cause: unknown): void {
  const message = cause instanceof Error ? cause.message : String(cause);
  // L128: the picker needs the debugger, which DevTools holds, and never closes it.
  toast(message.includes("devtools_open") ? "Close DevTools to pick an element for the chat." : message, { tone: "error" });
}

function togglePicker(id: string): void {
  const driven = driverOf(id) !== null;
  userUsed(id);
  if (devOf(useBrowserDev.getState().tabs, id).picking) void stopPicking(id).catch(failed);
  else void startPicking(id, driven).catch(failed);
}

/** The toolbar's developer button. */
export function BrowserDevButton({ id, ready }: { id: string; ready: boolean }) {
  const dev = useBrowserDev((s) => devOf(s.tabs, id));
  if (!hasDevTools()) return null;
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <TooltipIconButton
          tooltip="Developer tools"
          size="icon-lg"
          disabled={!ready}
          className={cn("shrink-0 rounded-full @max-md/browser-toolbar:hidden", (dev.devtools || dev.picking || dev.device) && "text-primary")}
        >
          <Terminal />
        </TooltipIconButton>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" data-slot="browser-dev-menu">
        <DropdownMenuItem onSelect={() => togglePicker(id)}>{dev.picking ? "Stop picking" : "Pick an element"}</DropdownMenuItem>
        <DropdownMenuCheckboxItem checked={dev.devtools} onCheckedChange={() => changeDev(id, { devtools: !dev.devtools })}>DevTools</DropdownMenuCheckboxItem>
        <DropdownMenuCheckboxItem checked={dev.device !== null} onCheckedChange={() => toggleDevice(id)}>Device sizes</DropdownMenuCheckboxItem>
        {dev.devtools && (
          <>
            <DropdownMenuSeparator />
            <DropdownMenuItem onSelect={() => changeDev(id, { dock: dev.dock === "right" ? "bottom" : "right" })}>
              {dev.dock === "right" ? "Dock DevTools below" : "Dock DevTools beside"}
            </DropdownMenuItem>
          </>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/** The strip while the picker is on, and with what it picked. */
export function BrowserPickerBar({ id }: { id: string }) {
  const { picking, picked } = useBrowserDev((s) => devOf(s.tabs, id));
  if (picking) {
    return (
      <Banner data-slot="browser-picking" text="Click an element of the page. Esc leaves.">
        <Button size="sm" variant="ghost" onClick={() => void stopPicking(id).catch(failed)}>Stop</Button>
      </Banner>
    );
  }
  if (!picked) return null;
  const toChat = () => {
    const session = sessionWith(id);
    const text = elementText(picked);
    if (session && typeInto(session, (draft) => (draft ? `${draft}\n\n${text}\n` : `${text}\n`))) changeDev(id, { picked: null });
    else toast("There is no chat open to add it to.", { tone: "error" });
  };
  const size = `${Math.round(picked.rect.width)}×${Math.round(picked.rect.height)}`;
  return (
    <Banner
      data-slot="browser-picked"
      text={<><span className="font-medium">{picked.component ?? picked.tag}</span> <span className="text-muted-foreground font-mono">{picked.selector}</span> <span className="text-muted-foreground tabular-nums">{size}</span></>}
      onDismiss={() => changeDev(id, { picked: null })}
    >
      <Button size="sm" onClick={toChat}>Add to chat</Button>
    </Banner>
  );
}

/** Above the floating composer, where it would lie over this element. */
function usableBounds(element: HTMLElement): BrowserBounds {
  const box = element.getBoundingClientRect();
  const composer = element.closest('[data-slot="pane-workspace"]')?.querySelector('[data-slot="floating-composer"]')?.getBoundingClientRect();
  const height = composer && composer.height > 0 ? Math.min(box.height, Math.max(0, composer.top - box.top - 12)) : box.height;
  return { x: box.left, y: box.top, width: box.width, height };
}

/** Around the tab's page area: where it is laid out as a device, and DevTools beside or below it. */
export function BrowserDevFrame({ id, active, children }: { id: string; active: boolean; children: ReactNode }) {
  const { devtools, dock, share, device } = useBrowserDev((s) => devOf(s.tabs, id));
  const ready = useBrowsers((s) => s.pages[id]?.ready === true);
  const openInMain = useAgentPause((s) => id in s.devtools);
  const stage = useRef<HTMLDivElement>(null);
  const pane = useRef<HTMLDivElement>(null);
  const [room, setRoom] = useState({ width: 0, height: 0 });

  useLayoutEffect(() => {
    const element = stage.current;
    if (!element || !device) return;
    const measure = () => {
      const { width, height } = usableBounds(element);
      setRoom({ width: width - 24, height: height - 24 });
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, [device]);

  const scale = device ? fitScale(device, room) : 1;
  useDeviceSize(id, device, scale, ready);

  // The page area, as big as the device shows.
  useLayoutEffect(() => {
    const area = stage.current?.querySelector<HTMLElement>('[data-slot="browser-page"]');
    if (!area) return;
    if (device) {
      area.style.flex = "none";
      area.style.width = `${Math.round(device.width * scale)}px`;
      area.style.height = `${Math.round(device.height * scale)}px`;
    } else {
      area.style.removeProperty("flex");
      area.style.removeProperty("width");
      area.style.removeProperty("height");
    }
  }, [device, scale]);

  // DevTools' pane follows its place as the tab's page follows its area.
  const opened = useRef(false);
  const hole = `devtools:${id}`;
  useEffect(() => {
    const element = pane.current;
    if (!devtools || !ready || !element) return;
    if (!opened.current) {
      opened.current = true;
      void openDevTools(id, usableBounds(element)).catch((error: unknown) => {
        opened.current = false;
        changeDev(id, { devtools: false });
        failed(error);
      });
    }
    if (!active) return;
    let placed = "";
    const place = () => {
      const bounds = element.closest("[inert]") ? null : usableBounds(element);
      const key = JSON.stringify(bounds);
      if (key === placed) return;
      placed = key;
      void placeDevTools(id, bounds).catch(() => {});
      showPage(hole, bounds);
    };
    const stop = followLayout(() => [element], place);
    return () => {
      stop();
      void placeDevTools(id, null).catch(() => {});
      showPage(hole, null);
    };
  }, [id, hole, devtools, ready, active, dock]);

  // The toggle turned off, or DevTools closed itself: the pane goes, and the keyboard back to the page.
  const wasOpen = useRef(false);
  useEffect(() => {
    if (wasOpen.current && !openInMain && devtools) changeDev(id, { devtools: false });
    wasOpen.current = openInMain;
  }, [id, openInMain, devtools]);
  useEffect(() => {
    if (devtools || !opened.current) return;
    opened.current = false;
    void closeDevTools(id).catch(failed);
    void browserFocus(id).catch(() => {});
  }, [id, devtools]);

  // Over the divider's gap there is no page: a drag stays with it.
  const drag = (event: React.PointerEvent<HTMLDivElement>) => {
    const whole = event.currentTarget.parentElement?.getBoundingClientRect();
    if (!whole) return;
    const handle = event.currentTarget;
    handle.setPointerCapture(event.pointerId);
    const move = (moved: PointerEvent) => {
      const part = dock === "right" ? (whole.right - moved.clientX) / whole.width : (whole.bottom - moved.clientY) / whole.height;
      changeDev(id, { share: Math.min(0.8, Math.max(0.15, part)) });
    };
    const done = () => {
      handle.removeEventListener("pointermove", move);
      handle.removeEventListener("pointerup", done);
    };
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", done);
  };

  if (!hasDevTools()) return <>{children}</>;
  return (
    <div data-slot="browser-dev-frame" className={cn("flex min-h-0 w-full flex-1", dock === "bottom" && "flex-col")}>
      <div ref={stage} className={cn("flex min-h-0 min-w-0 flex-1", device && "bg-muted/60 items-center justify-center overflow-hidden")}>
        {children}
      </div>
      {devtools && (
        <>
          <div
            role="separator"
            aria-orientation={dock === "right" ? "vertical" : "horizontal"}
            aria-label="Resize DevTools"
            onPointerDown={drag}
            className={cn("bg-border hover:bg-primary shrink-0", dock === "right" ? "w-1 cursor-col-resize" : "h-1 cursor-row-resize")}
          />
          <div ref={pane} data-slot="browser-devtools" className="shrink-0" style={dock === "right" ? { width: `${share * 100}%` } : { height: `${share * 100}%` }} />
        </>
      )}
    </div>
  );
}
