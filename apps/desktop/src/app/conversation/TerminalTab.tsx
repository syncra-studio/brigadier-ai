import {
  ChevronDown,
  Plus,
  Terminal as TerminalIcon,
  X,
} from "@openai/apps-sdk-ui/components/Icon";
import {
  lazy,
  type RefObject,
  Suspense,
  useCallback,
  useEffect,
  useRef,
  useState,
} from "react";

import { useReveal } from "@/app/conversation/SidePanel";
import { TitlebarButton, TitlebarTips } from "@/components/titlebar-button";
import { request } from "@/ipc/client";
import { tokenPx } from "@/lib/tokens";
import { cn } from "@/lib/utils";
import { changedPaneSize, savedPaneSizes } from "@/state/paneSizes";
import { useApp } from "@/state/store";
import {
  addTab,
  closeTab,
  hasTab,
  noteShell,
  noteShellCwd,
  noteShellTitle,
  noteTabReader,
  placeConversation,
  selectTab,
  setTerminalOpen,
  tabNames,
  forgetRestoredOutput,
  noteShellExit,
  restoredOutput,
  terminalPlace,
  useTerminalPlace,
} from "@/state/terminalPlaces";

// The terminal itself (xterm) loads when a shell first shows, not with the app.
const TerminalView = lazy(() =>
  import("@/app/conversation/TerminalView").then((module) => ({ default: module.TerminalView })),
);

/**
 * The bottom terminal (⌘J, ⌃`): shells the daemon runs, in the thread's checkout or folder,
 * or Home's. They keep running while the pane is hidden; closing a tab ends its shell.
 */


const MIN_HEIGHT = 160;
const DEFAULT_HEIGHT = 280;

/** The pane's height, shared by every place and kept across launches. `present` is whether
 * the pane is in the page (it is while its place has tabs), so its column can be watched. */
function useTerminalHeight(pane: RefObject<HTMLElement | null>, present: boolean) {
  const [saved, setSaved] = useState(() => savedPaneSizes().terminal ?? null);
  const [room, setRoom] = useState(() => window.innerHeight);
  useEffect(() => {
    const column = present ? pane.current?.parentElement : null;
    if (!column) return;
    const observer = new ResizeObserver(() => setRoom(column.getBoundingClientRect().height));
    observer.observe(column);
    return () => observer.disconnect();
  }, [pane, present]);
  const max = Math.max(MIN_HEIGHT, room * 0.5);
  const height = Math.max(MIN_HEIGHT, Math.min(saved ?? DEFAULT_HEIGHT, max));
  const resize = useCallback((next: number | null) => {
    changedPaneSize(savedPaneSizes(), "terminal", next);
    setSaved(next);
  }, []);
  return { height, max, resize };
}

/**
 * The bottom terminal of a place (a conversation's, or Home's): its tabs, each an independent
 * shell. Hiding the pane never ends a shell.
 */
export function TerminalPane({ place }: { place: string }) {
  const data = useTerminalPlace(place);
  const open = data.open;
  const start = useRef<{ y: number; height: number } | null>(null);
  const pane = useRef<HTMLElement>(null);
  const { height, max, resize } = useTerminalHeight(pane, data.tabs.length > 0);
  const previousFocus = useRef<HTMLElement | null>(null);
  const wasOpen = useRef(false);
  useEffect(() => {
    if (open && !wasOpen.current) {
      previousFocus.current =
        document.activeElement instanceof HTMLElement ? document.activeElement : null;
    } else if (!open && wasOpen.current && pane.current?.contains(document.activeElement)) {
      const editor = document.querySelector<HTMLElement>(
        '[data-slot="composer"] [contenteditable="true"], [data-slot="composer"] textarea',
      );
      (editor ?? previousFocus.current)?.focus();
    }
    wasOpen.current = open;
  }, [open]);
  const reveal = useReveal(open);
  // A pane found open on arrival (coming back to its place) leaves the keyboard where it is; it
  // takes it once it is opened or its tab changes here.
  type Arrival = { open: boolean; active: string | null };
  const [arrival, setArrival] = useState<Arrival | null>({ open, active: data.active });
  const [arrivedAt, setArrivedAt] = useState(place);
  if (arrivedAt !== place) {
    setArrivedAt(place);
    setArrival({ open, active: data.active });
  } else if (arrival && (arrival.open !== open || arrival.active !== data.active)) {
    setArrival(null);
  }
  const projectPath = useApp((s) => {
    const conversationId = placeConversation(place);
    const projectId = conversationId ? s.conversations[conversationId]?.projectId : null;
    return projectId ? s.projects[projectId]?.repos[0]?.path : undefined;
  });
  const names = tabNames(data.tabs, projectPath);
  // With too many tabs for their names, they show only their icons.
  const strip = useRef<HTMLDivElement>(null);
  const [compactTabs, setCompactTabs] = useState(false);
  useEffect(() => {
    const element = strip.current;
    if (!open || !data.active || !element) return;
    const fitTabs = () => {
      const selected = element.querySelector<HTMLElement>('[aria-selected="true"]');
      setCompactTabs(
        (selected?.parentElement?.getBoundingClientRect().width ?? 0) <
          tokenPx("--spacing-terminal-tab-label-min"),
      );
      selected?.parentElement?.scrollIntoView({ block: "nearest", inline: "nearest" });
    };
    fitTabs();
    const observer = new ResizeObserver(fitTabs);
    observer.observe(element);
    return () => observer.disconnect();
  }, [open, data.active]);
  const hide = useCallback(() => setTerminalOpen(place, false), [place]);
  useEffect(() => {
    if (!open) return;
    const key = (event: KeyboardEvent) => {
      if (!pane.current?.contains(document.activeElement)) return;
      if (!event.metaKey || event.ctrlKey || event.altKey) return;
      const { tabs, active } = terminalPlace(place);
      if (event.shiftKey && ["BracketLeft", "BracketRight"].includes(event.code) && tabs.length) {
        event.preventDefault();
        const index = tabs.findIndex((tab) => tab.id === active);
        const next = (index + (event.code === "BracketLeft" ? -1 : 1) + tabs.length) % tabs.length;
        selectTab(place, tabs[next]!.id);
        return;
      }
      if (event.shiftKey) return;
      if (event.code === "KeyT") {
        event.preventDefault();
        addTab(place);
      }
      if (event.code === "KeyW" && active) {
        event.preventDefault();
        closeTab(place, active);
      }
    };
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  }, [open, place]);
  if (!reveal.mounted && !data.tabs.length) return null;
  return (
    <section
      ref={pane}
      data-slot="terminal-pane"
      aria-label="Terminal"
      inert={!open}
      className={cn(
        "bg-terminal-background relative flex min-h-0 shrink-0 flex-col",
        reveal.moving && "ease-panel transition-[height] duration-500 motion-reduce:transition-none",
      )}
      style={{
        height: reveal.out ? height : 0,
        visibility: reveal.mounted ? "visible" : "hidden",
      }}
    >
      <div
        role="separator"
        aria-label="Resize terminal"
        aria-orientation="horizontal"
        tabIndex={0}
        aria-valuenow={height}
        aria-valuemin={MIN_HEIGHT}
        aria-valuemax={max}
        className="group/resize absolute inset-x-0 -top-2 z-10 h-4 cursor-row-resize outline-none"
        onPointerDown={(event) => {
          if (event.button !== 0) return;
          event.preventDefault();
          event.currentTarget.setPointerCapture(event.pointerId);
          start.current = { y: event.clientY, height };
        }}
        onPointerMove={(event) => {
          if (!start.current) return;
          const wanted = start.current.height + start.current.y - event.clientY;
          if (wanted < MIN_HEIGHT) {
            // Dragged shut: it opens again at the height it had before the drag.
            resize(start.current.height);
            start.current = null;
            hide();
          } else resize(Math.min(wanted, max));
        }}
        onPointerUp={(event) => {
          start.current = null;
          if (event.currentTarget.hasPointerCapture(event.pointerId))
            event.currentTarget.releasePointerCapture(event.pointerId);
        }}
        onPointerCancel={() => {
          start.current = null;
        }}
        onDoubleClick={() => resize(null)}
        onKeyDown={(event) => {
          const value =
            event.key === "ArrowUp"
              ? height + 16
              : event.key === "ArrowDown"
                ? height - 16
                : event.key === "Home"
                  ? MIN_HEIGHT
                  : event.key === "End"
                    ? max
                    : null;
          if (value === null) return;
          event.preventDefault();
          resize(Math.max(MIN_HEIGHT, Math.min(value, max)));
        }}
      >
        <span className="bg-input absolute inset-x-0 top-1 h-px opacity-0 group-hover/resize:opacity-100 group-focus-visible/resize:opacity-100" />
      </div>
      <div className="border-terminal-divider flex h-full min-h-0 flex-col overflow-hidden border-t">
        <header
          data-slot="terminal-header"
          className="border-terminal-divider h-terminal-header flex shrink-0 items-center gap-1 border-b px-2"
        >
          <div
            ref={strip}
            role="tablist"
            aria-label="Terminals"
            tabIndex={-1}
            className="hide-scrollbar flex min-w-0 scroll-px-1 gap-0.5 overflow-x-auto"
            style={{
              width: `calc(var(--spacing-terminal-tab) * ${data.tabs.length || 1} + var(--spacing) * 0.5 * ${Math.max(0, data.tabs.length - 1)})`,
            }}
            onKeyDown={(event) => {
              if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
              if (!data.tabs.length) return;
              event.preventDefault();
              const index = data.tabs.findIndex((tab) => tab.id === data.active);
              const next =
                event.key === "Home"
                  ? 0
                  : event.key === "End"
                    ? data.tabs.length - 1
                    : (index + (event.key === "ArrowLeft" ? -1 : 1) + data.tabs.length) %
                      data.tabs.length;
              const id = data.tabs[next]!.id;
              selectTab(place, id);
              requestAnimationFrame(() =>
                strip.current?.querySelector<HTMLButtonElement>(`[data-tab="${id}"]`)?.focus(),
              );
            }}
          >
            {data.tabs.map((tab, index) => {
              const selected = tab.id === data.active;
              return (
                <div
                  key={tab.id}
                  className={cn(
                    "group/terminal-tab h-terminal-tab-height min-w-terminal-tab-min max-w-terminal-tab ps-terminal-tab-start pe-terminal-tab-end font-terminal-tab flex flex-1 items-center gap-1 rounded-lg py-1 text-[length:var(--text-terminal-tab)] leading-[var(--text-terminal-tab--line-height)]",
                    selected
                      ? "bg-terminal-tab text-terminal-tab-active shadow-terminal-tab"
                      : "text-terminal-tab-inactive hover:bg-toolbar-hover",
                    compactTabs && "px-1",
                  )}
                >
                  <button
                    type="button"
                    role="tab"
                    data-tab={tab.id}
                    id={`terminal-tab-${tab.id}`}
                    aria-controls={`terminal-panel-${tab.id}`}
                    aria-selected={selected}
                    tabIndex={selected ? 0 : -1}
                    aria-label={names[index]}
                    onClick={() => selectTab(place, tab.id)}
                    className="focus-visible:ring-ring flex min-w-0 flex-1 items-center gap-1 rounded-sm outline-none focus-visible:ring-1"
                  >
                    <TerminalIcon aria-hidden className="size-icon-md shrink-0" />
                    <TabLabel key={names[index]} name={names[index]!} hidden={compactTabs} />
                  </button>
                  <button
                    type="button"
                    aria-label={`Close ${names[index]} tab`}
                    onClick={() => closeTab(place, tab.id)}
                    className={cn(
                      "text-terminal-tab-inactive hover:bg-toolbar-hover hover:text-terminal-tab-active focus-visible:ring-ring flex size-5 shrink-0 items-center justify-center rounded-full outline-none focus-visible:ring-1",
                      !selected &&
                        "hidden group-focus-within/terminal-tab:flex group-hover/terminal-tab:flex",
                    )}
                  >
                    <X aria-hidden className="size-icon-xs" />
                  </button>
                </div>
              );
            })}
          </div>
          <TitlebarTips>
            <TitlebarButton
              tooltip="New terminal"
              shortcut="⌘T"
              className="shrink-0"
              onClick={() => addTab(place)}
            >
              <Plus />
            </TitlebarButton>
            <div className="flex-1" />
            <TitlebarButton tooltip="Hide terminal" shortcut="⌘J" className="shrink-0" onClick={hide}>
              <ChevronDown />
            </TitlebarButton>
          </TitlebarTips>
        </header>
        {data.tabs.map((tab) => (
          <div
            key={tab.id}
            id={`terminal-panel-${tab.id}`}
            role="tabpanel"
            aria-labelledby={`terminal-tab-${tab.id}`}
            className={tab.id === data.active ? "flex min-h-0 flex-1 flex-col" : "hidden"}
          >
            <TerminalTab
              place={place}
              tabId={tab.id}
              active={open && !arrival && tab.id === data.active}
            />
          </div>
        ))}
      </div>
    </section>
  );
}

/** A tab's name; clipped, it fades at the edge rather than losing its end to dots. */
function TabLabel({ name, hidden }: { name: string; hidden: boolean }) {
  const label = useRef<HTMLSpanElement>(null);
  const [overflowing, setOverflowing] = useState(false);
  useEffect(() => {
    const element = label.current;
    if (!element) return;
    const measure = () => setOverflowing(element.scrollWidth > element.clientWidth);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, []);
  return (
    <span
      ref={label}
      className={cn(
        "min-w-0 flex-1 overflow-hidden text-start whitespace-nowrap",
        hidden && "hidden",
      )}
      style={{
        maskImage: overflowing
          ? "linear-gradient(to right, black calc(100% - var(--spacing-terminal-label-fade)), transparent)"
          : undefined,
      }}
    >
      {name}
    </span>
  );
}

/** One tab's shell: started (or found again) in the daemon when the tab first shows. */
export function TerminalTab({
  place,
  tabId,
  active,
}: {
  place: string;
  tabId: string;
  /** Takes the keyboard. */
  active: boolean;
}) {
  const density = useApp((s) => s.settings.density);
  const open = useCallback(
    async (cols: number, rows: number) => {
      const conversationId = placeConversation(place);
      const selection = useApp.getState().selection;
      const projectId =
        conversationId === null && selection.type === "draft" && selection.kind === "session"
          ? selection.projectId
          : null;
      const { terminal } = await request({
        method: "openTerminal",
        ...(conversationId ? { conversationId } : {}),
        ...(projectId ? { projectId } : {}),
        sessionId: tabId,
        cols,
        rows,
      });
      if (!hasTab(place, tabId)) {
        void request({ method: "closeTerminal", terminalId: terminal.id }).catch(() => {});
        throw new Error("Terminal closed");
      }
      noteShell(place, tabId, terminal.id);
      noteShellCwd(place, tabId, terminal.cwd);
      return terminal;
    },
    [place, tabId],
  );
  const onTitle = useCallback(
    (title: string) => noteShellTitle(place, tabId, title),
    [place, tabId],
  );
  const reader = useCallback((read: () => string) => noteTabReader(tabId, read), [tabId]);
  // An exit the store missed (it came before the shell's id did) still closes the tab.
  const onExit = useCallback(() => noteShellExit(place, tabId), [place, tabId]);
  const onClear = useCallback(() => forgetRestoredOutput(tabId), [tabId]);
  // What the tab showed before it was closed, when it is one brought back.
  const restored = useCallback(() => restoredOutput(tabId), [tabId]);
  return (
    <Suspense fallback={null}>
      <TerminalView
        key={density}
        open={open}
        focus={active}
        restored={restored}
        onTitle={onTitle}
        reader={reader}
        onExit={onExit}
        onClear={onClear}
      />
    </Suspense>
  );
}
