import {
  Branch,
  Folders,
  Globe,
  PlusCircle,
  Terminal,
  X,
} from "@openai/apps-sdk-ui/components/Icon";
import {
  createContext,
  type CSSProperties,
  type FC,
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

import { listen } from "@tauri-apps/api/event";

import { WORKERS_LABEL, WorkersTab } from "@/app/conversation/Agents";
import type { FileTarget } from "@/app/conversation/FilesTab";
import type { AgentsPanelState } from "@/app/conversation/WorkerChip";
import { DiffGlyph } from "@/components/assistant-ui/elements/diff-glyph";
import { TitlebarButton, TitlebarTips } from "@/components/titlebar-button";
import { useSidebar } from "@/components/ui/sidebar";
import { tokenPx } from "@/lib/tokens";
import { cn } from "@/lib/utils";
import { takePaneClose } from "@/state/closedPanes";
import { newBrowserTab, reopenBrowserTab } from "@/state/browsers";
import { undoTerminalClose } from "@/state/terminalSessions";
import { changedPaneSize, savedPaneSizes } from "@/state/paneSizes";
import { useApp } from "@/state/store";

const ReviewTab = lazy(() =>
  import("@/app/conversation/ReviewTab").then((module) => ({
    default: module.ReviewTab,
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
const FilesTab = lazy(() =>
  import("@/app/conversation/FilesTab").then((module) => ({
    default: module.FilesTab,
  })),
);

/** Independent tools share one right-side slot; the terminal is a separate bottom pane. */

/** The kinds of tab the side panel opens. */
export type SideTab =
  | "workers"
  | "review"
  | "terminal"
  | "browser"
  | "files"
  | "source"
  | "sideChat";

/** Each tab's title, icon and shortcut (macOS keys; Ctrl for ⌘ elsewhere). */
const TABS: Record<
  SideTab,
  { title: string; icon: ReactNode; keys: string | null }
> = {
  workers: { title: WORKERS_LABEL, icon: null, keys: null },
  review: { title: "Review", icon: <DiffGlyph />, keys: "⌃⇧G" },
  terminal: { title: "Terminal", icon: <Terminal />, keys: "⌃`" },
  browser: { title: "Browser", icon: <Globe />, keys: "⌘T" },
  files: { title: "Files", icon: <Folders />, keys: "⌘P" },
  source: { title: "Source", icon: <Branch />, keys: null },
  sideChat: { title: "Side chat", icon: <PlusCircle />, keys: "⌥⌘S" },
};

/** The tabs the titlebar has a button for, in order. */
const TOOLS: readonly SideTab[] = [
  "files",
  "source",
  "sideChat",
  "terminal",
  "browser",
  "review",
];

/** The panel's own shortcuts: show or hide it, and full view. */

/** Which tab a key press opens: ⌃⇧G, ⌃`, ⌘T, ⌘P and ⌥⌘S (Ctrl for ⌘ off macOS). */
function tabForKey(event: KeyboardEvent, mac: boolean): SideTab | null {
  const command = mac ? event.metaKey : event.ctrlKey;
  if (
    event.ctrlKey &&
    event.shiftKey &&
    !event.altKey &&
    !event.metaKey &&
    event.code === "KeyG"
  ) {
    return "review";
  }
  if (event.ctrlKey && !event.shiftKey && !event.altKey && !event.metaKey) {
    if (event.code === "Backquote") return "terminal";
  }
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
  active: Exclude<SideTab, "terminal"> | null;
  fullscreen: boolean;
  terminalOpen: boolean;
};

const CLOSED: PanelState = {
  open: false,
  active: null,
  fullscreen: false,
  terminalOpen: false,
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
  terminalHeight: number;
  terminalMaxHeight: number;
  resizeTerminal: (height: number | null) => void;
  workspace: (element: HTMLElement | null) => void;
  file: FileTarget | null;
  openFile: (file: FileTarget | null) => void;
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
  terminalHeight: 280,
  terminalMaxHeight: 400,
  resizeTerminal: () => {},
  workspace: () => {},
  file: null,
  openFile: () => {},
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
  const [file, setFile] = useState<FileTarget | null>(null);
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
  const terminalHeight = Math.max(
    160,
    Math.min(sizes.terminal ?? 280, room.height * 0.5),
  );
  const available = useMemo<SideTab[]>(
    () =>
      kind === "session"
        ? [
            "workers",
            "review",
            "terminal",
            "browser",
            "files",
            "source",
            "sideChat",
          ]
        : kind === "chat"
          ? ["sideChat"]
          : [],
    [kind],
  );
  const openTab = useCallback((tab: SideTab) => {
    setState((current) =>
      tab === "terminal"
        ? { ...current, terminalOpen: true }
        : { ...current, open: true, active: tab, fullscreen: !fitsNow.current },
    );
  }, []);
  const hide = useCallback(
    () =>
      setState((current) => ({ ...current, open: false, fullscreen: false })),
    [],
  );
  const closeTab = useCallback(
    (tab: SideTab) =>
      setState((current) =>
        tab === "terminal"
          ? { ...current, terminalOpen: false }
          : current.active === tab
            ? { ...current, open: false, fullscreen: false }
            : current,
      ),
    [],
  );
  const toggleTab = useCallback(
    (tab: SideTab) =>
      setState((current) =>
        tab === "terminal"
          ? { ...current, terminalOpen: !current.terminalOpen }
          : current.open && current.active === tab
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
  // Native menu accelerators keep working while the isolated browser owns keyboard focus.
  useEffect(() => {
    if (!mac || kind === "sideChat") return;
    let disposed = false;
    let unsubscribe: (() => void) | undefined;
    const keys: Record<
      string,
      { code: string; shift?: boolean; control?: boolean }
    > = {
      terminal: { code: "KeyJ" },
      "terminal-alternate": { code: "Backquote", control: true },
      new: { code: "KeyT" },
      reopen: { code: "KeyT", shift: true },
      address: { code: "KeyL" },
      full: { code: "KeyF", shift: true },
      close: { code: "KeyW" },
      previous: { code: "BracketLeft", shift: true },
      next: { code: "BracketRight", shift: true },
    };
    void listen<string>("pane-shortcut", ({ payload }) => {
      const key = keys[payload];
      if (!key) return;
      const browser =
        state.open && state.active === "browser"
          ? document.querySelector('[data-pane="browser"]')
          : null;
      const target =
        (!document.hasFocus() && browser) || document.activeElement || window;
      target.dispatchEvent(
        new KeyboardEvent("keydown", {
          bubbles: true,
          cancelable: true,
          code: key.code,
          metaKey: !key.control,
          ctrlKey: key.control ?? false,
          shiftKey: key.shift ?? false,
        }),
      );
    })
      .then((off) => {
        if (disposed) off();
        else unsubscribe = off;
      })
      .catch(() => {});
    return () => {
      disposed = true;
      unsubscribe?.();
    };
  }, [mac, kind, state.open, state.active]);
  // A mounted view can change conversation without carrying its selected worker/file over.
  const [scope, setScope] = useState(conversationId);
  if (scope !== conversationId) {
    setScope(conversationId);
    setState(CLOSED);
    setWorker(null);
    setFile(null);
  }
  useEffect(() => {
    if (kind === "sideChat") return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented) return;
      const command = mac ? event.metaKey : event.ctrlKey;
      if (
        command &&
        event.shiftKey &&
        !event.altKey &&
        event.code === "KeyT" &&
        conversationId
      ) {
        const closed = takePaneClose(conversationId);
        if (closed === "terminal" && undoTerminalClose(conversationId)) {
          event.preventDefault();
          openTab("terminal");
          return;
        }
        if (closed === "browser" && reopenBrowserTab(conversationId)) {
          event.preventDefault();
          openTab("browser");
          return;
        }
      }
      if (
        command &&
        !event.altKey &&
        !event.shiftKey &&
        event.code === "KeyJ" &&
        available.includes("terminal")
      ) {
        event.preventDefault();
        toggleTab("terminal");
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
          '[data-slot="terminal-pane"], [data-pane="browser"]',
        )
      )
        return;
      event.preventDefault();
      if (tab === "files") setFile(null);
      if (tab === "browser" && conversationId) {
        newBrowserTab(conversationId);
        openTab(tab);
      } else toggleTab(tab);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [kind, mac, available, toggleTab, openTab, state.open, conversationId]);
  const panel = useMemo<SidePanelApi>(
    () => ({
      state,
      visible,
      fits,
      width,
      limits,
      resize: (next) => {
        if (state.active)
          setSizes((current) => changedPaneSize(current, state.active!, next));
      },
      composerWidth,
      composerLimits,
      resizeComposer: (next) =>
        setSizes((current) =>
          changedPaneSize(current, "browserComposer", next),
        ),
      terminalHeight,
      terminalMaxHeight: Math.max(160, room.height * 0.5),
      resizeTerminal: (next) =>
        setSizes((current) => changedPaneSize(current, "terminal", next)),
      workspace,
      file,
      openFile: (next) => {
        setFile(next);
        openTab("files");
      },
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
      terminalHeight,
      composerWidth,
      composerLimits,
      room.height,
      workspace,
      file,
      available,
      hide,
      openTab,
      toggleTab,
      closeTab,
      reveal,
      buttonsWidth,
    ],
  );
  useEffect(() => {
    const restore = (event: Event) => {
      if ((event as CustomEvent<string>).detail === conversationId)
        openTab("terminal");
    };
    document.addEventListener("brigadier:restore-terminal", restore);
    return () =>
      document.removeEventListener("brigadier:restore-terminal", restore);
  }, [conversationId, openTab]);
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
        {TOOLS.filter((tab) => available.includes(tab)).map((tab) => (
          <TitlebarButton
            key={tab}
            tooltip={TABS[tab].title}
            shortcut={keys(TABS[tab].keys)}
            aria-pressed={
              tab === "terminal"
                ? state.terminalOpen
                : visible && state.active === tab
            }
            onClick={() => toggleTab(tab)}
          >
            {TABS[tab].icon}
          </TitlebarButton>
        ))}
      </div>
    </TitlebarTips>
  );
};

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

/** The Source tab, for now: where source control will be. */
function SourceTab() {
  return (
    <div className="m-auto flex max-w-xs flex-col items-center gap-2 p-4 text-center">
      <Branch className="text-muted-foreground size-icon-lg" />
      <p className="text-sm font-medium">Source control</p>
      <p className="text-muted-foreground text-sm">
        Changes, commits and branches will show here. Until then, Review shows
        what changed.
      </p>
    </div>
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
  const { state: sidebar } = useSidebar();
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
                full && sidebar === "collapsed" && "ps-titlebar-clear",
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
            {state.active === "workers" && conversationId ? (
              <WorkersTab conversationId={conversationId} />
            ) : state.active === "review" && conversationId ? (
              <Suspense fallback={null}>
                <ReviewTab conversationId={conversationId} />
              </Suspense>
            ) : state.active === "browser" && conversationId ? (
              <Suspense fallback={null}>
                <BrowserTab conversationId={conversationId} />
              </Suspense>
            ) : state.active === "sideChat" && conversationId ? (
              <Suspense fallback={null}>
                <SideChatTab conversationId={conversationId} />
              </Suspense>
            ) : state.active === "files" && conversationId ? (
              <Suspense fallback={null}>
                <FilesTab conversationId={conversationId} />
              </Suspense>
            ) : state.active === "source" ? (
              <SourceTab />
            ) : null}
          </div>
        </aside>
      </div>
      {!full && out && <Splitter />}
    </div>
  );
}
