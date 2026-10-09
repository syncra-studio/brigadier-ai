import { Terminal } from "@openai/apps-sdk-ui/components/Icon";
import { useEffect, useMemo, useState } from "react";
import { BarItem } from "@/app/BarItem";
import { useRightSidebar } from "@/app/conversation/RightSidebar";
import type { AgentsPanelState } from "@/app/conversation/WorkerChip";
import { TitlebarButton } from "@/components/titlebar-button";
import { takePaneClose } from "@/state/closedPanes";
import { openReviewTab, reopenTab } from "@/state/sessionTabs";
import { HOME_PLACE, placeOf, terminalWorksHere, toggleTerminal, undoTabClose, useTerminalPlaces } from "@/state/terminalPlaces";
import { useApp } from "@/state/store";

export function shortcutLabel(keys: string, mac: boolean): string {
  return mac
    ? keys
    : keys
        .replace("⌃", "Ctrl+")
        .replace("⌥", "Alt+")
        .replace("⇧", "Shift+")
        .replace("⌘", "Ctrl+");
}

/** Shared right-sidebar selection and shortcuts; there is no legacy side-panel slot. */
export function useSidePanel(conversationId: string | null, enabled: boolean, embedded = false) {
  const rightSidebar = useRightSidebar(conversationId, enabled);
  const [worker, setWorker] = useState<string | null>(null);
  const [scope, setScope] = useState(conversationId);
  if (scope !== conversationId) { setScope(conversationId); setWorker(null); }
  const mac = useApp((s) => s.info?.platform === "macos");
  const { openTab, setOpen, open, active } = rightSidebar;
  useEffect(() => {
    if (embedded) return;
    const key = (event: KeyboardEvent) => {
      if (event.defaultPrevented || event.isComposing) return;
      const command = mac ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey;
      if (!enabled && command && event.shiftKey && !event.altKey && event.code === "KeyT") {
        const closed = takePaneClose(conversationId ?? HOME_PLACE);
        if (closed === "terminal") undoTabClose(conversationId ? `conv:${conversationId}` : HOME_PLACE);
        if (closed === "tab" && enabled && conversationId) reopenTab(conversationId);
        if (closed) event.preventDefault();
      }
      if (!enabled || !conversationId) return;
      if (command && !event.shiftKey && !event.altKey && event.code === "KeyP") {
        event.preventDefault(); openTab("files", true);
      }
      if (event.ctrlKey && event.shiftKey && !event.metaKey && !event.altKey && event.code === "KeyG") {
        event.preventDefault(); openReviewTab(conversationId, { type: "all" });
      }
    };
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  }, [conversationId, enabled, embedded, mac, openTab]);
  const agents = useMemo(() => ({
    panel: open && active === "workers" ? worker : undefined,
    setPanel: (next: AgentsPanelState) => {
      if (next === undefined) { if (active === "workers") setOpen(false); return; }
      setWorker(next); openTab("workers");
    },
  }), [open, active, worker, setOpen, openTab]);
  return { panel: rightSidebar, agents };
}

export function TerminalButton() {
  const mac = useApp((s) => s.info?.platform === "macos");
  const works = useApp(terminalWorksHere);
  const place = useApp((s) => placeOf(s.selection));
  const open = useTerminalPlaces((s) => s.places[place]?.open ?? false);
  return (
    <BarItem show={works}>
      <TitlebarButton
        tooltip="Terminal"
        shortcut={shortcutLabel("⌘J", mac)}
        aria-pressed={open}
        onClick={() => toggleTerminal()}
      >
        <Terminal />
      </TitlebarButton>
    </BarItem>
  );
}

const MOTION_MS = 500;
/** Whether the panel is mounted, whether it is out at its width, and whether it is moving. */
type Reveal = { mounted: boolean; out: boolean; moving: boolean };

/** The panel's reveal, following `visible`: it mounts closed and opens a frame later, and
 * stays mounted until it has closed. */
export function useReveal(visible: boolean): Reveal {
  const [seen, setSeen] = useState(visible);
  const [mounted, setMounted] = useState(visible);
  const [out, setOut] = useState(visible);
  const [moving, setMoving] = useState(false);
  if (seen !== visible) {
    setSeen(visible);
    if (visible) setMounted(true);
    else setMoving(true);
  }
  useEffect(() => {
    // Two frames: the first lays it out where it is with its transition on, the second moves
    // it (a width changed in the same frame as its transition is turned on doesn't animate).
    let frame = requestAnimationFrame(() => {
      frame = requestAnimationFrame(() => {
        setMoving(true);
        setOut(visible);
      });
    });
    // Settled once the motion has run, counting the frames before it starts.
    const settled = window.setTimeout(() => {
      setMoving(false);
      if (!visible) setMounted(false);
    }, MOTION_MS + 100);
    return () => {
      cancelAnimationFrame(frame);
      window.clearTimeout(settled);
    };
  }, [visible]);
  return { mounted, out, moving };
}

