import {
  Branch,
  Folders,
  Globe,
  Lightbulb,
  PlusCircle,
  Terminal,
  X,
} from "@openai/apps-sdk-ui/components/Icon";
import {
  createContext,
  type CSSProperties,
  type FC,
  Fragment,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
  lazy,
  Suspense,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import { BarItem } from "@/app/BarItem";
import { WORKERS_LABEL, WorkersTab } from "@/app/conversation/Agents";
import type { AgentsPanelState } from "@/app/conversation/WorkerChip";
import { TitlebarButton, TitlebarTips } from "@/components/titlebar-button";
import { useSidebar } from "@/components/ui/sidebar";
import { tokenPx } from "@/lib/tokens";
import { cn } from "@/lib/utils";
import { takePaneClose } from "@/state/closedPanes";
import { newBrowserTab, reopenBrowserTab } from "@/state/browsers";
import { openReviewTab, reopenTab } from "@/state/sessionTabs";
import {
  HOME_PLACE,
  placeOf,
  terminalWorksHere,
  toggleTerminal,
  undoTabClose,
  useTerminalPlaces,
} from "@/state/terminalPlaces";
import { changedPaneSize, savedPaneSizes, withSavedTerminal } from "@/state/paneSizes";
import { useApp } from "@/state/store";

const RunningTab = lazy(() =>
  import("@/app/conversation/RunningTab").then((module) => ({ default: module.RunningTab })),
);
const PlanTab = lazy(() =>
  import("@/app/conversation/PlanTab").then((module) => ({
    default: module.PlanTab,
  })),
);
const SideChatTab = lazy(() =>
  import("@/app/conversation/SideChatTab").then((module) => ({
    default: module.SideChatTab,
  })),
);
const BrowserTab = lazy(() =>
  import("@/app/conversation/BrowserTab").then((module) => ({
    default: module.BrowserTab,
  })),
);
const SourcePanel = lazy(() =>
  import("@/app/conversation/SourcePanel").then((module) => ({
    default: module.SourcePanel,
  })),
);
const FilesTab = lazy(() =>
  import("@/app/conversation/FilesTab").then((module) => ({
    default: module.FilesTab,
  })),
);

/** Independent tools share one right-side slot; the terminal is a separate bottom pane
 * (`TerminalPane`). */

/** The kinds of tab the side panel opens. */
export type SideTab =
  | "running"
  | "workers"
  | "browser"
  | "files"
  | "source"
  | "sideChat"
  /** The plan opened from its card in the thread (`showPlanDoc`). */
  | "plan";

/** Each tab's title, icon and shortcut (macOS keys; Ctrl for ⌘ elsewhere). */
const TABS: Record<
  SideTab,
  { title: string; icon: ReactNode; keys: string | null }
> = {
  running: { title: "Running", icon: null, keys: null },
  workers: { title: WORKERS_LABEL, icon: null, keys: null },
  browser: { title: "Browser", icon: <Globe />, keys: "⌘T" },
  files: { title: "Files", icon: <Folders />, keys: "⌘P" },
  source: { title: "Source", icon: <Branch />, keys: null },
  sideChat: { title: "Side chat", icon: <PlusCircle />, keys: "⌥⌘S" },
  plan: { title: "Plan", icon: <Lightbulb />, keys: null },
};

/** The tabs the titlebar has a button for, in order; Terminal comes before Browser. Review is
 * one of a session's main tabs. */
const TOOLS: readonly SideTab[] = ["files", "source", "sideChat", "browser"];

/** The panel's own shortcuts: show or hide it, and full view. */

/** ⌃⇧G: a session's Review tab, on every change. */
function isReviewKey(event: KeyboardEvent): boolean {
  return (
    event.ctrlKey && event.shiftKey && !event.altKey && !event.metaKey && event.code === "KeyG"
  );
}

/** Which tab a key press opens: ⌘T, ⌘P and ⌥⌘S (Ctrl for ⌘ off macOS). */
function tabForKey(event: KeyboardEvent, mac: boolean): SideTab | null {
  const command = mac ? event.metaKey : event.ctrlKey;
  if (command && !event.shiftKey && !event.altKey && event.code === "KeyT")
    return "browser";
  if (command && !event.shiftKey && !event.altKey && event.code === "KeyP")
    return "files";
  if (command && event.altKey && !event.shiftKey && event.code === "KeyS")
    return "sideChat";
  return null;
}

/** A shortcut as this platform writes it. */
export function shortcutLabel(keys: string, mac: boolean): string {
  return mac
    ? keys
    : keys
        .replace("⌃", "Ctrl+")
        .replace("⌥", "Alt+")
        .replace("⇧", "Shift+")
        .replace("⌘", "Ctrl+");
}

type PanelState = {
  open: boolean;
  active: SideTab | null;
  fullscreen: boolean;
};

const CLOSED: PanelState = {
  open: false,
  active: null,
  fullscreen: false,
};
const MOTION_MS = 500;
/** The room the panel has: the workspace beside the rail and sidebar, and the window. */
type Room = { workspace: number; window: number; height: number };

/** The panel's least and greatest width in a workspace this wide. */
function widthLimits(workspace: number): { min: number; max: number } {
  const min = tokenPx("--spacing-side-panel-min");
  return {
    min,
    max: Math.max(min, workspace - tokenPx("--spacing-side-panel-chat-min")),
  };
}

/**
 * The panel's width: the dragged share of the workspace, or by default the preferred width,
 * or more where the window is tall and the thread keeps its room; always within its limits.
 */
export function panelWidth(share: number | null, room: Room): number {
  const { min, max } = widthLimits(room.workspace);
  const wanted =
    share === null
      ? Math.max(
          min,
          Math.min(
            room.height * 1.6,
            room.workspace - tokenPx("--spacing-side-panel-chat-room"),
          ),
          Math.min(
            tokenPx("--spacing-side-panel-preferred"),
            room.workspace - tokenPx("--spacing-side-panel-chat-min"),
          ),
        )
      : share * room.workspace;
  return Math.round(Math.min(max, Math.max(min, wanted)));
}

/**
 * Whether the panel fits beside the thread: the window is not the narrowest, not narrow
 * while the sidebar is open, and the workspace holds both the thread's and the panel's least.
 */
export function panelFits(room: Room, sidebarOpen: boolean): boolean {
  if (room.workspace === 0) return true;
  if (room.window < tokenPx("--spacing-narrowest-window")) return false;
  if (sidebarOpen && room.window < tokenPx("--spacing-narrow-window"))
    return false;
  return (
    room.workspace >=
    tokenPx("--spacing-side-panel-min") +
      tokenPx("--spacing-side-panel-chat-min")
  );
}

export type SidePanelApi = {
  state: PanelState;
  visible: boolean;
  fits: boolean;
  width: number;
  limits: { min: number; max: number };
  resize: (width: number | null) => void;
  composerWidth: number;
  composerLimits: { min: number; max: number };
  resizeComposer: (width: number | null) => void;
  workspace: (element: HTMLElement | null) => void;
  available: readonly SideTab[];
  hide: () => void;
  openTab: (tab: SideTab) => void;
  toggleTab: (tab: SideTab) => void;
  closeTab: (tab: SideTab) => void;
  setFullscreen: (fullscreen: boolean) => void;
  reveal: Reveal;
  buttonsWidth: number;
  setButtonsWidth: (width: number) => void;
};

export const SidePanelContext = createContext<SidePanelApi>({
  state: CLOSED,
  visible: false,
  fits: true,
  width: 0,
  limits: { min: 0, max: 0 },
  resize: () => {},
  composerWidth: 572,
  composerLimits: { min: 372, max: 917 },
  resizeComposer: () => {},
  workspace: () => {},
  available: [],
  hide: () => {},
  openTab: () => {},
  toggleTab: () => {},
  closeTab: () => {},
  setFullscreen: () => {},
  reveal: { mounted: false, out: false, moving: false },
  buttonsWidth: 0,
  setButtonsWidth: () => {},
});

/** Brings the main area back from under the panel's full view, for a tab opened from it. */
export function useShowMain(): () => void {
  const { state, setFullscreen } = useContext(SidePanelContext);
  return () => {
    if (state.fullscreen) setFullscreen(false);
  };
}

/** The room around `element`, followed as it and the window resize. */
function useRoom(): {
  room: Room;
  workspace: (element: HTMLElement | null) => void;
} {
  const [room, setRoom] = useState<Room>(() => ({
    workspace: 0,
    window: window.innerWidth,
    height: window.innerHeight,
  }));
  const observer = useRef<ResizeObserver | null>(null);
  const workspace = useCallback((element: HTMLElement | null) => {
    observer.current?.disconnect();
    observer.current = null;
    if (!element) return;
    const measure = () =>
      setRoom({
        workspace: element.getBoundingClientRect().width,
        window: window.innerWidth,
        height: element.getBoundingClientRect().height,
      });
    observer.current = new ResizeObserver(measure);
    observer.current.observe(element);
  }, []);
  // A window growing only taller leaves the workspace's width alone.
  useEffect(() => {
    const onResize = () =>
      setRoom((current) => ({ ...current, window: window.innerWidth }));
    window.addEventListener("resize", onResize);
    return () => {
      window.removeEventListener("resize", onResize);
      observer.current?.disconnect();
    };
  }, []);
  return { room, workspace };
}

/**
 * The open conversation's side panel, and the Workers tab's selection behind
 * `AgentsPanelContext` (opening a worker opens its tab).
 */
export function useSidePanel(
  conversationId: string | null,
  kind: "session" | "chat" | "sideChat" | null,
): {
  panel: SidePanelApi;
  agents: {
    panel: AgentsPanelState;
    setPanel: (panel: AgentsPanelState) => void;
  };
} {
  const [state, setState] = useState<PanelState>(CLOSED);
  const [worker, setWorker] = useState<string | null>(null);
  const [sizes, setSizes] = useState(savedPaneSizes);
  const [buttonsWidth, setButtonsWidth] = useState(0);
  const mac = useApp((s) => s.info?.platform === "macos");
  const { open: sidebarOpen } = useSidebar();
  const { room, workspace } = useRoom();
  const fits = panelFits(room, sidebarOpen);
  const fitsNow = useRef(fits);
  useEffect(() => {
    fitsNow.current = fits;
  }, [fits]);
  const visible = state.open && (fits || state.fullscreen);
  const reveal = useReveal(visible);
  const limits = useMemo(() => widthLimits(room.workspace), [room.workspace]);
  const preferred = state.active
    ? (sizes[state.active] ??
      tokenPx(
        state.active === "workers"
          ? "--spacing-workers-pane"
          : "--spacing-browser-pane",
      ))
    : undefined;
  const width =
    preferred === undefined
      ? panelWidth(null, room)
      : Math.round(Math.min(limits.max, Math.max(limits.min, preferred)));
  const composerLimits = useMemo(() => {
    const max = Math.max(0, room.workspace - 20);
    return { min: Math.min(372, max), max };
  }, [room.workspace]);
  const composerWidth = Math.min(
    composerLimits.max,
    Math.max(composerLimits.min, sizes.browserComposer ?? 572),
  );
  const archived = useApp(
    (s) =>
      conversationId !== null &&
      s.conversations[conversationId]?.lifecycle === "archived",
  );
  // A session's tools need its checkout; a draft, like Home, has the Browser only. An archived
  // thread takes no side chat, as it takes no terminal.
  const available = useMemo<SideTab[]>(() => {
    const tabs: SideTab[] =
      kind === "session" && conversationId
        ? ["workers", "running", "browser", "files", "source", "sideChat", "plan"]
        : kind === "chat"
          ? ["sideChat", "browser"]
          : kind === "sideChat"
            ? []
            : ["browser"];
    return archived ? tabs.filter((tab) => tab !== "sideChat") : tabs;
  }, [kind, conversationId, archived]);
  // Archiving the thread closes its open side chat.
  if (archived && state.open && state.active === "sideChat")
    setState((current) => ({ ...current, open: false, fullscreen: false }));
  // Home's and the drafts' pages are Home's, as their terminals are.
  const browserId = conversationId ?? HOME_PLACE;
  const openTab = useCallback((tab: SideTab) => {
    setState((current) => ({
      ...current,
      open: true,
      active: tab,
      fullscreen: !fitsNow.current,
    }));
  }, []);
  const hide = useCallback(
    () =>
      setState((current) => ({ ...current, open: false, fullscreen: false })),
    [],
  );
  const closeTab = useCallback(
    (tab: SideTab) =>
      setState((current) =>
        current.active === tab
          ? { ...current, open: false, fullscreen: false }
          : current,
      ),
    [],
  );
  const toggleTab = useCallback(
    (tab: SideTab) =>
      setState((current) =>
        current.open && current.active === tab
          ? { ...current, open: false, fullscreen: false }
          : {
              ...current,
              open: true,
              active: tab,
              fullscreen: !fitsNow.current,
            },
      ),
    [],
  );
  // A mounted view can change conversation without carrying its selected worker/file over.
  const [scope, setScope] = useState(conversationId);
  if (scope !== conversationId) {
    setScope(conversationId);
    setState(CLOSED);
    setWorker(null);
  }
  useEffect(() => {
    if (kind === "sideChat") return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented) return;
      const command = mac ? event.metaKey : event.ctrlKey;
      if (command && event.shiftKey && !event.altKey && event.code === "KeyT") {
        const closed = takePaneClose(browserId);
        const place = conversationId ? `conv:${conversationId}` : HOME_PLACE;
        if (closed === "terminal" && undoTabClose(place)) {
          event.preventDefault();
          return;
        }
        if (closed === "browser" && reopenBrowserTab(browserId)) {
          event.preventDefault();
          openTab("browser");
          return;
        }
        if (closed === "tab" && conversationId && reopenTab(conversationId)) {
          event.preventDefault();
          return;
        }
      }
      if (isReviewKey(event) && kind === "session" && conversationId) {
        event.preventDefault();
        openReviewTab(conversationId, { type: "all" });
        // The Review tab is in the main area, which a panel in full view hides.
        setState((current) => (current.fullscreen ? { ...current, fullscreen: false } : current));
        return;
      }
      if (command && event.shiftKey && !event.altKey && event.code === "KeyF") {
        if (!state.open) return;
        event.preventDefault();
        setState((current) => ({
          ...current,
          fullscreen: !current.fullscreen,
        }));
        return;
      }
      const tab = tabForKey(event, mac);
      if (!tab || !available.includes(tab)) return;
      // The active terminal/browser owns Command-T for a new session/page.
      if (
        tab === "browser" &&
        event.target instanceof Element &&
        event.target.closest(
          '[data-slot="terminal-pane"], [data-terminal-menu], [data-pane="browser"]',
        )
      )
        return;
      event.preventDefault();
      if (tab === "browser") {
        newBrowserTab(browserId);
        openTab(tab);
      } else toggleTab(tab);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [kind, mac, available, toggleTab, openTab, state.open, conversationId, browserId]);
  const panel = useMemo<SidePanelApi>(
    () => ({
      state,
      visible,
      fits,
      width,
      limits,
      resize: (next) => {
        if (state.active)
          setSizes((current) =>
            changedPaneSize(withSavedTerminal(current), state.active!, next),
          );
      },
      composerWidth,
      composerLimits,
      resizeComposer: (next) =>
        setSizes((current) =>
          changedPaneSize(withSavedTerminal(current), "browserComposer", next),
        ),
      workspace,
      available,
      hide,
      openTab,
      toggleTab,
      closeTab,
      setFullscreen: (fullscreen) =>
        setState((current) => ({ ...current, fullscreen })),
      reveal,
      buttonsWidth,
      setButtonsWidth,
    }),
    [
      state,
      visible,
      fits,
      width,
      limits,
      composerWidth,
      composerLimits,
      workspace,
      available,
      hide,
      openTab,
      toggleTab,
      closeTab,
      reveal,
      buttonsWidth,
    ],
  );
  const workersOpen = state.open && state.active === "workers";
  const agents = useMemo(
    () => ({
      // Keep the detail selection while another pane takes the slot.
      panel: worker,
      setPanel: (next: AgentsPanelState) => {
        if (next === undefined) {
          closeTab("workers");
          return;
        }
        setWorker(next);
        openTab("workers");
      },
    }),
    [worker, openTab, closeTab],
  );
  // Consumers use undefined to identify a closed workers pane, but its selection remains saved.
  return {
    panel,
    agents: { ...agents, panel: workersOpen ? worker : undefined },
  };
}

/**
 * The splitter on the panel's start edge: drag to resize (below half the least width hides
 * the panel), double-click for the default width; arrow keys step it, Home and End go to its
 * least and greatest.
 */
function Splitter() {
  const { width, limits, resize, hide, setFullscreen } =
    useContext(SidePanelContext);
  const start = useRef<{ x: number; width: number } | null>(null);
  const [resizing, setResizing] = useState(false);

  const end = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (!start.current) return;
    start.current = null;
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
    setResizing(false);
  };

  return (
    <div
      role="separator"
      aria-orientation="vertical"
      aria-label="Resize side panel"
      aria-valuenow={width}
      aria-valuemin={limits.min}
      aria-valuemax={limits.max}
      tabIndex={0}
      data-resizing={resizing || undefined}
      className="group/resize w-resize-handle top-titlebar absolute bottom-0 start-0 z-10 flex -translate-x-1/2 cursor-col-resize justify-center outline-none rtl:translate-x-1/2"
      onPointerDown={(event) => {
        if (event.button !== 0) return;
        event.preventDefault();
        event.currentTarget.setPointerCapture(event.pointerId);
        start.current = { x: event.clientX, width };
        setResizing(true);
      }}
      onPointerMove={(event) => {
        if (!start.current) return;
        const rtl = getComputedStyle(event.currentTarget).direction === "rtl";
        const wanted =
          start.current.width +
          (start.current.x - event.clientX) * (rtl ? -1 : 1);
        if (wanted < limits.min / 2) {
          // Dragged shut: it opens again at the width it had before the drag.
          resize(start.current.width);
          end(event);
          hide();
          return;
        }
        resize(Math.min(limits.max, Math.max(limits.min, wanted)));
        if (wanted > limits.max) {
          end(event);
          setFullscreen(true);
        }
      }}
      onPointerUp={end}
      onPointerCancel={end}
      onDoubleClick={() => resize(null)}
      onKeyDown={(event) => {
        const step = 10;
        const next =
          event.key === "ArrowLeft"
            ? width + step
            : event.key === "ArrowRight"
              ? width - step
              : event.key === "Home"
                ? limits.min
                : event.key === "End"
                  ? limits.max
                  : null;
        if (next === null) return;
        event.preventDefault();
        resize(Math.min(limits.max, Math.max(limits.min, next)));
      }}
    >
      <span className="bg-input w-px opacity-0 transition-opacity duration-150 group-hover/resize:opacity-100 group-focus-visible/resize:opacity-100 group-data-resizing/resize:opacity-100" />
    </div>
  );
}

/** Each tool toggles its own pane. Split view keeps these beside the conversation title;
 * full view keeps them at the window's end, with room in the browser header. */
export const PanelButtons: FC = () => {
  const { state, visible, width, available, toggleTab, setButtonsWidth } =
    useContext(SidePanelContext);
  const mac = useApp((s) => s.info?.platform === "macos");
  const keys = (value: string | null) =>
    value ? shortcutLabel(value, mac) : undefined;
  const group = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const element = group.current;
    if (!element) return;
    const observer = new ResizeObserver(() =>
      setButtonsWidth(element.offsetWidth),
    );
    observer.observe(element);
    return () => observer.disconnect();
  }, [setButtonsWidth]);
  return (
    <TitlebarTips>
      <div
        ref={group}
        data-slot="panel-buttons"
        data-tauri-drag-region
        className="h-titlebar absolute top-0 z-20 flex items-center gap-1.5"
        style={{ insetInlineEnd: visible && !state.fullscreen ? width + 4 : 4 }}
      >
        {TOOLS.map((tab) => {
          const button = (
            <TitlebarButton
              tooltip={TABS[tab].title}
              shortcut={keys(TABS[tab].keys)}
              aria-pressed={visible && state.active === tab}
              onClick={() => toggleTab(tab)}
            >
              {TABS[tab].icon}
            </TitlebarButton>
          );
          return (
            <Fragment key={tab}>
              {tab === "browser" && <TerminalButton covered={visible && state.fullscreen} />}
              {/* Side chat comes and goes with where it works; the others stay with the view. */}
              {tab === "sideChat" ? (
                <BarItem show={available.includes(tab)}>{button}</BarItem>
              ) : (
                available.includes(tab) && button
              )}
            </Fragment>
          );
        })}
      </div>
    </TitlebarTips>
  );
};

/**
 * Terminal, beside the panel's tools: there wherever a terminal works (not in an archived
 * thread), pressed while the place's terminal shows. Full view hides it (`covered`); pressing
 * Terminal then leaves full view.
 */
export function TerminalButton({ covered = false }: { covered?: boolean }) {
  const mac = useApp((s) => s.info?.platform === "macos");
  const works = useApp(terminalWorksHere);
  const place = useApp((s) => placeOf(s.selection));
  const open = useTerminalPlaces((s) => s.places[place]?.open ?? false);
  return (
    <BarItem show={works}>
      <TitlebarButton
        tooltip="Terminal"
        shortcut={shortcutLabel("⌘J", mac)}
        aria-pressed={open && !covered}
        onClick={() => toggleTerminal()}
      >
        <Terminal />
      </TitlebarButton>
    </BarItem>
  );
}

/** Room for tools in the conversation titlebar, or the full-view pane header. */
export function PanelButtonsRoom({
  besidePanel = false,
}: {
  besidePanel?: boolean;
}) {
  const { buttonsWidth, reveal, state } = useContext(SidePanelContext);
  const width = besidePanel || state.fullscreen ? buttonsWidth : 0;
  return (
    <div
      aria-hidden
      className={cn(
        "shrink-0",
        besidePanel &&
          reveal.moving &&
          "ease-panel transition-[width] duration-500 motion-reduce:transition-none",
      )}
      style={{ width: `${width}px` }}
    />
  );
}

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

/** The side panel of a conversation, while it shows (and while it opens and closes). */
export function SidePanel({
  conversationId,
}: {
  conversationId: string | null;
}) {
  const { state, visible, width, hide, reveal } = useContext(SidePanelContext);
  const { mounted, out, moving } = reveal;
  if (!mounted || !state.active) return null;
  const full = state.fullscreen && visible;
  const size = { width: `${width}px` } satisfies CSSProperties;
  return (
    <div
      className={cn(
        "relative flex h-full",
        full ? "min-w-0 flex-1" : "shrink-0",
      )}
    >
      <div
        data-slot="side-panel-clip"
        inert={!visible}
        className={cn(
          "flex h-full justify-end overflow-clip",
          full ? "min-w-0 flex-1" : "shadow-side-panel",
          moving &&
            !full &&
            "ease-panel transition-[width] duration-500 motion-reduce:transition-none",
        )}
        style={full ? undefined : out ? size : { width: 0 }}
      >
        <aside
          aria-label={TABS[state.active].title}
          data-pane={state.active}
          className={cn(
            "flex h-full shrink-0 flex-col",
            full ? "w-full" : "column-divider",
          )}
          style={full ? undefined : size}
        >
          {state.active !== "browser" && (
            <header
              data-tauri-drag-region
              className={cn(
                "h-titlebar flex shrink-0 items-center gap-2 px-3",
                full && "ps-clear-3",
              )}
            >
              <h2 className="min-w-0 flex-1 truncate text-sm font-medium">
                {TABS[state.active].title}
              </h2>
              <TitlebarTips>
                <TitlebarButton
                  tooltip={`Close ${TABS[state.active].title}`}
                  onClick={hide}
                >
                  <X />
                </TitlebarButton>
              </TitlebarTips>
              <PanelButtonsRoom />
            </header>
          )}
          <div className="flex min-h-0 flex-1 flex-col">
            {state.active === "running" && conversationId ? (
              <Suspense fallback={null}>
                <RunningTab conversationId={conversationId} />
              </Suspense>
            ) : state.active === "workers" && conversationId ? (
              <WorkersTab conversationId={conversationId} />
            ) : state.active === "browser" ? (
              <Suspense fallback={null}>
                <BrowserTab conversationId={conversationId ?? HOME_PLACE} />
              </Suspense>
            ) : state.active === "sideChat" && conversationId ? (
              <Suspense fallback={null}>
                <SideChatTab conversationId={conversationId} />
              </Suspense>
            ) : state.active === "files" && conversationId ? (
              <Suspense fallback={null}>
                <FilesTab conversationId={conversationId} />
              </Suspense>
            ) : state.active === "source" && conversationId ? (
              <Suspense fallback={null}>
                <SourcePanel conversationId={conversationId} />
              </Suspense>
            ) : state.active === "plan" && conversationId ? (
              <Suspense fallback={null}>
                <PlanTab conversationId={conversationId} />
              </Suspense>
            ) : null}
          </div>
        </aside>
      </div>
      {!full && out && <Splitter />}
    </div>
  );
}
