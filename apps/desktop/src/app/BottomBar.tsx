import { PlusCircle, Settings, Terminal } from "@openai/apps-sdk-ui/components/Icon";
import { useEffect, useState, type ReactNode } from "react";

import { KeepAwakeMenu, UsageMenu } from "@/app/BarStatus";
import { useShortcuts } from "@/app/shortcuts";
import { BarButton } from "@/app/sidebar/nav";
import { cn } from "@/lib/utils";
import { toggleSettings } from "@/state/actions";
import { useBarActions } from "@/state/barActions";
import { useApp } from "@/state/store";
import { placeOf, toggleTerminal, useTerminalPlaces } from "@/state/terminalPlaces";

/**
 * The bar along the window's bottom, on the window chrome under the sidebar panel and the
 * content alike, and always there: Settings, the agents' usage and keeping awake at its start;
 * what is only sometimes there (updates, Side chat) and the terminal at its end.
 */
export function BottomBar() {
  const inSettings = useApp((s) => s.selection.type === "settings");
  const shortcuts = useShortcuts();
  const sideChat = useBarActions((s) => s.sideChat);
  const place = useApp((s) => placeOf(s.selection));
  const terminalOn = useTerminalPlaces((s) => s.places[place]?.open ?? false);
  return (
    <footer
      aria-label="App bar"
      data-tauri-drag-region
      className="h-bottom-bar bg-chrome flex shrink-0 items-center gap-0.5 px-2"
    >
      <BarButton
        label={inSettings ? "Close settings" : "Settings"}
        shortcut={shortcuts.settings}
        selected={inSettings}
        onClick={toggleSettings}
      >
        <Settings />
      </BarButton>
      <UsageMenu />
      <KeepAwakeMenu />
      <div data-tauri-drag-region className="min-w-0 flex-1 self-stretch" />
      <BarItem show={sideChat !== null}>
        <BarButton
          label="Side chat"
          shortcut={shortcuts.sideChat}
          selected={sideChat?.on ?? false}
          onClick={() => useBarActions.getState().sideChat?.toggle()}
        >
          <PlusCircle />
        </BarButton>
      </BarItem>
      <BarButton
        label="Terminal"
        shortcut={shortcuts.terminal}
        selected={terminalOn}
        onClick={() => toggleTerminal()}
      >
        <Terminal />
      </BarButton>
    </footer>
  );
}

const ITEM_MS = 200;

function prefersReducedMotion(): boolean {
  return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

/**
 * Something on the bar that is only sometimes there: it grows from nothing and fades in, and
 * shrinks and fades out, over 200ms, so its neighbours glide. Leaving, it can't be focused or
 * clicked at once; it is gone when the motion ends (at once with reduced motion). Shown again
 * while leaving, it turns back.
 */
export function BarItem({ show, children }: { show: boolean; children: ReactNode }) {
  const [seen, setSeen] = useState(show);
  const [mounted, setMounted] = useState(show);
  const [open, setOpen] = useState(show);
  if (seen !== show) {
    setSeen(show);
    const instant = prefersReducedMotion();
    if (show) {
      setMounted(true);
      if (instant) setOpen(true);
    } else {
      setOpen(false);
      if (instant) setMounted(false);
    }
  }
  useEffect(() => {
    if (show) {
      // Laid out closed first, so the opening has somewhere to start from.
      let frame = requestAnimationFrame(() => {
        frame = requestAnimationFrame(() => setOpen(true));
      });
      return () => cancelAnimationFrame(frame);
    }
    const timer = window.setTimeout(() => setMounted(false), ITEM_MS);
    return () => window.clearTimeout(timer);
  }, [show]);
  if (!mounted) return null;
  return (
    <div
      inert={!show}
      aria-hidden={!show || undefined}
      className={cn(
        "ease-standard grid shrink-0 transition-[grid-template-columns,opacity] duration-200 motion-reduce:transition-none",
        open ? "grid-cols-[1fr] opacity-100" : "grid-cols-[0fr] opacity-0",
      )}
    >
      <div className="flex min-w-0 items-center overflow-hidden">{children}</div>
    </div>
  );
}
