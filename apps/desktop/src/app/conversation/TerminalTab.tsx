import "@xterm/xterm/css/xterm.css";

import { ClipboardAddon } from "@xterm/addon-clipboard";
import { FitAddon } from "@xterm/addon-fit";
import { SerializeAddon } from "@xterm/addon-serialize";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { type ITheme, Terminal } from "@xterm/xterm";
import {
  ChevronDown,
  Plus,
  Terminal as TerminalIcon,
  X,
} from "@openai/apps-sdk-ui/components/Icon";
import { type RefObject, useCallback, useEffect, useRef, useState } from "react";

import { useReveal } from "@/app/conversation/SidePanel";
import { TitlebarButton, TitlebarTips } from "@/components/titlebar-button";
import { openUrl, request } from "@/ipc/client";
import type { TerminalInfo, TerminalOutput } from "@/ipc/generated";
import { tokenColor, tokenPx } from "@/lib/tokens";
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
  onTerminalOutput,
  placeConversation,
  selectTab,
  setTerminalOpen,
  tabNames,
  takeRestoredOutput,
  terminalPlace,
  useTerminalPlace,
} from "@/state/terminalPlaces";

/**
 * The bottom terminal (⌘J, ⌃`): shells the daemon runs, in the thread's checkout or folder,
 * or Home's. They keep running while the pane is hidden; closing a tab ends its shell.
 */

const ANSI = [
  "black",
  "red",
  "green",
  "yellow",
  "blue",
  "magenta",
  "cyan",
  "white",
] as const;

/** The terminal in the app's colours: the page and its text, the scrim for selection, the
 * palette's 16 colours, and a scrollbar thumb in the border colour. */
function theme(): ITheme {
  const tokens: `--${string}`[] = [
    "--terminal-background",
    "--terminal-foreground",
    "--scrim",
    "--border",
    ...ANSI.map((name) => `--ansi-${name}` as const),
    ...ANSI.map((name) => `--ansi-bright-${name}` as const),
  ];
  const [background = "", foreground = "", scrim = "", border = "", ...ansi] =
    tokens.map(tokenColor);
  const colors = Object.fromEntries([
    ...ANSI.map((name, index) => [name, ansi[index]]),
    ...ANSI.map((name, index) => [
      `bright${name[0]!.toUpperCase()}${name.slice(1)}`,
      ansi[index + ANSI.length],
    ]),
  ]) as ITheme;
  return {
    ...colors,
    background,
    foreground,
    cursor: foreground,
    cursorAccent: background,
    selectionBackground: scrim,
    selectionInactiveBackground: scrim,
    scrollbarSliderBackground: border,
    scrollbarSliderHoverBackground: border,
    scrollbarSliderActiveBackground: border,
    overviewRulerBorder: background,
  };
}

/** OSC 52: shells may set the clipboard, but never read it. */
const WRITE_ONLY_CLIPBOARD = {
  readText: () => "",
  writeText: async (_selection: unknown, text: string) => {
    await navigator.clipboard.writeText(text);
  },
};

const DIM = "\u001b[2m";
const RESET = "\u001b[0m";

const MIN_HEIGHT = 160;
const DEFAULT_HEIGHT = 280;

/** The pane's height, shared by every place and kept across launches. */
function useTerminalHeight(pane: RefObject<HTMLElement | null>) {
  const [saved, setSaved] = useState(() => savedPaneSizes().terminal ?? null);
  const [room, setRoom] = useState(() => window.innerHeight);
  useEffect(() => {
    const column = pane.current?.parentElement;
    if (!column) return;
    const observer = new ResizeObserver(() => setRoom(column.getBoundingClientRect().height));
    observer.observe(column);
    return () => observer.disconnect();
  }, [pane]);
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
  const { height, max, resize } = useTerminalHeight(pane);
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
  // What the tab showed before it was closed, when it is one brought back.
  const [restored] = useState(() => takeRestoredOutput(tabId));
  return (
    <TerminalView
      key={density}
      open={open}
      focus={active}
      restored={restored}
      onTitle={onTitle}
      reader={reader}
    />
  );
}

/**
 * A shell the daemon runs, shown live: `open` starts it (or re-attaches to it) at the view's
 * size. When it ends, `onExit` hears its exit code (a pane's tab just closes). `restored` is
 * output shown above the shell's, `onTitle` hears the titles the shell sets, and `reader`
 * learns how to read what the view shows, until the function it returns is called.
 */
export function TerminalView({
  open,
  onExit,
  restored = null,
  onTitle,
  reader,
  focus = true,
  className,
}: {
  open: (cols: number, rows: number) => Promise<TerminalInfo>;
  onExit?: (code: number | null) => void;
  restored?: string | null;
  onTitle?: (title: string) => void;
  reader?: (read: () => string) => () => void;
  /** Takes the keyboard once it is open. */
  focus?: boolean;
  className?: string;
}) {
  const host = useRef<HTMLDivElement>(null);
  const liveTerminal = useRef<Terminal | null>(null);
  const focusNow = useRef(focus);
  useEffect(() => {
    focusNow.current = focus;
    if (focus) liveTerminal.current?.focus();
  }, [focus]);
  const connected = useApp((s) => s.connection.status === "connected");
  const mac = useApp((s) => s.info?.platform === "macos");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const element = host.current;
    if (!connected || !element) return;
    // The type is the host's (font-terminal, text-terminal): the system's monospace face.
    const style = getComputedStyle(element);
    const terminal = new Terminal({
      fontFamily: style.fontFamily,
      fontSize: Number.parseFloat(style.fontSize),
      lineHeight: 1.2,
      letterSpacing: 0,
      theme: theme(),
      cursorStyle: "bar",
      cursorInactiveStyle: "bar",
      cursorBlink: true,
      scrollback: 5000,
      overviewRuler: { width: 10 },
      // OSC 8 links.
      linkHandler: {
        activate: (_event, uri) => {
          void openUrl(uri).catch(() => {});
        },
      },
    });
    liveTerminal.current = terminal;
    const fit = new FitAddon();
    const serialize = new SerializeAddon();
    terminal.loadAddon(fit);
    terminal.loadAddon(serialize);
    // Addresses in plain text.
    terminal.loadAddon(
      new WebLinksAddon((_event, uri) => {
        void openUrl(uri).catch(() => {});
      }),
    );
    terminal.loadAddon(new ClipboardAddon(undefined, WRITE_ONLY_CLIPBOARD));
    terminal.open(element);
    fit.fit();
    if (restored) terminal.write(`${restored}${RESET}\r\n`);
    const title = onTitle ? terminal.onTitleChange(onTitle) : null;
    const unread = reader?.(() => serialize.serialize({ scrollback: 5000 }));

    let id: string | null = null;
    let ended = false;
    // Output can arrive before `openTerminal` answers; it waits here until then.
    const early: TerminalOutput[] = [];
    const show = (output: TerminalOutput) => {
      if (output.type === "data") {
        terminal.write(output.data);
        return;
      }
      ended = true;
      if (!onExit) return;
      const code = output.code === null ? "" : ` with code ${output.code}`;
      terminal.write(`\r\n${DIM}[Process exited${code}.]${RESET}\r\n`);
      onExit(output.code);
    };
    const stop = onTerminalOutput((output) => {
      if (id === null) early.push(output);
      else if (output.terminalId === id) show(output);
    });
    let live = true;
    open(terminal.cols, terminal.rows)
      .then((opened) => {
        if (!live) return;
        id = opened.id;
        setError(null);
        terminal.write(opened.scrollback);
        for (const output of early.splice(0))
          if (output.terminalId === id) show(output);
        if (focusNow.current) terminal.focus();
      })
      .catch((cause: unknown) => {
        if (live)
          setError(cause instanceof Error ? cause.message : String(cause));
      });

    terminal.attachCustomKeyEventHandler((event) => {
      if (event.type !== "keydown") return true;
      // ⌘K clears the terminal (not the app's search).
      if (
        mac &&
        event.metaKey &&
        !event.ctrlKey &&
        !event.altKey &&
        !event.shiftKey &&
        event.code === "KeyK"
      ) {
        event.preventDefault();
        event.stopPropagation();
        terminal.clear();
        // The daemon forgets it too, so reopening the tab doesn't bring it back.
        if (id)
          void request({ method: "clearTerminal", terminalId: id }).catch(
            () => {},
          );
        return false;
      }
      // Off macOS, Ctrl+J, K, T and W are the shell's while it has the keyboard.
      if (
        !mac &&
        event.ctrlKey &&
        !event.metaKey &&
        !event.altKey &&
        !event.shiftKey &&
        ["KeyJ", "KeyK", "KeyT", "KeyW"].includes(event.code)
      ) {
        event.stopPropagation();
        return true;
      }
      const copy =
        !mac &&
        ((event.ctrlKey &&
          (event.shiftKey || terminal.hasSelection()) &&
          event.code === "KeyC") ||
          (event.ctrlKey && event.code === "Insert"));
      const paste =
        !mac &&
        ((event.ctrlKey && event.shiftKey && event.code === "KeyV") ||
          (event.shiftKey && event.code === "Insert"));
      if (copy) {
        event.preventDefault();
        void navigator.clipboard
          .writeText(terminal.getSelection())
          .catch(() => {});
        return false;
      }
      if (paste) {
        event.preventDefault();
        void navigator.clipboard
          .readText()
          .then((text) => terminal.paste(text))
          .catch(() => {});
        return false;
      }
      if (event.metaKey && !event.altKey && !event.ctrlKey && !event.shiftKey) {
        if (["KeyT", "KeyW"].includes(event.code)) return false;
        const editing: Record<string, string> = {
          ArrowLeft: "\x01",
          ArrowUp: "\x01",
          ArrowRight: "\x05",
          ArrowDown: "\x05",
          Backspace: "\x15",
          Delete: "\x0b",
        };
        if (editing[event.key] && id) {
          event.preventDefault();
          void request({
            method: "writeTerminal",
            terminalId: id,
            data: editing[event.key]!,
          });
          return false;
        }
        // ⌘C and ⌘V are the Edit menu's: the terminal copies its selection and pastes.
      }
      return !(
        event.shiftKey &&
        (event.metaKey || event.ctrlKey) &&
        event.code === "KeyT"
      );
    });
    const input = terminal.onData((data) => {
      if (id && !ended) {
        request({ method: "writeTerminal", terminalId: id, data }).catch(
          () => {},
        );
      }
    });
    const resize = terminal.onResize(({ cols, rows }) => {
      if (id && !ended) {
        request({ method: "resizeTerminal", terminalId: id, cols, rows }).catch(
          () => {},
        );
      }
    });
    const observer = new ResizeObserver(() => fit.fit());
    observer.observe(element);
    return () => {
      live = false;
      observer.disconnect();
      input.dispose();
      resize.dispose();
      stop();
      title?.dispose();
      unread?.();
      liveTerminal.current = null;
      terminal.dispose();
    };
  }, [open, connected, onExit, mac, restored, onTitle, reader]);

  return (
    <div
      className={cn("bg-terminal-background flex min-h-0 flex-1 flex-col", className)}
    >
      {error && (
        <p role="alert" className="text-destructive shrink-0 px-4 py-2 text-sm">
          {error}
        </p>
      )}
      <div
        ref={host}
        data-slot="terminal"
        className="font-terminal text-terminal min-h-0 flex-1 ps-4 pe-2 pt-2 pb-3"
      />
    </div>
  );
}
