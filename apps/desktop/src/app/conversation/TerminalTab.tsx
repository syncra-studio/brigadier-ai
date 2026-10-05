import "@xterm/xterm/css/xterm.css";

import { FitAddon } from "@xterm/addon-fit";
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
import { cn } from "@/lib/utils";
import { changedPaneSize, savedPaneSizes } from "@/state/paneSizes";
import { useApp } from "@/state/store";
import {
  addTab,
  closeTab,
  hasTab,
  noteShell,
  noteShellCwd,
  onTerminalOutput,
  placeConversation,
  selectTab,
  setTerminalOpen,
  tabNames,
  terminalPlace,
  useTerminalPlace,
} from "@/state/terminalPlaces";

/**
 * The bottom terminal (⌘J, ⌃`): shells the daemon runs, in the thread's checkout or folder,
 * or Home's. They keep running while the pane is hidden; closing a tab ends its shell.
 */

/** A token colour in hex, which the terminal's renderer understands (tokens are oklch). */
function tokenColor(name: `--${string}`): string {
  const canvas = document.createElement("canvas");
  canvas.width = 1;
  canvas.height = 1;
  const context = canvas.getContext("2d", { willReadFrequently: true });
  if (!context) return "";
  context.fillStyle = getComputedStyle(document.documentElement)
    .getPropertyValue(name)
    .trim();
  context.fillRect(0, 0, 1, 1);
  const [red = 0, green = 0, blue = 0] = context.getImageData(0, 0, 1, 1).data;
  return `#${[red, green, blue].map((part) => part.toString(16).padStart(2, "0")).join("")}`;
}

function theme(): ITheme {
  return {
    background: tokenColor("--background"),
    foreground: tokenColor("--foreground"),
    cursor: tokenColor("--foreground"),
    cursorAccent: tokenColor("--background"),
    selectionBackground: tokenColor("--muted"),
  };
}

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
  const names = tabNames(data.tabs);
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
        "bg-background relative flex min-h-0 shrink-0 flex-col",
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
      <div className="border-border flex h-full min-h-0 flex-col overflow-hidden border-t">
        <TitlebarTips>
          <header className="border-border flex h-10 shrink-0 items-center gap-1 border-b px-2">
            <div role="tablist" aria-label="Terminals" className="flex min-w-0 items-center gap-1">
              {data.tabs.map((tab, index) => {
                const selected = tab.id === data.active;
                return (
                  <div
                    key={tab.id}
                    className={cn(
                      "group/tab flex h-8 w-60 min-w-16 shrink items-center gap-2 rounded-lg ps-2 pe-1 text-sm",
                      selected
                        ? "bg-panel-tab shadow-panel-tab text-foreground"
                        : "text-muted-foreground hover:bg-toolbar-hover hover:text-foreground",
                    )}
                  >
                    <button
                      type="button"
                      role="tab"
                      aria-selected={selected}
                      className="flex min-w-0 flex-1 items-center gap-2 self-stretch outline-none focus-visible:underline"
                      onClick={() => selectTab(place, tab.id)}
                    >
                      <TerminalIcon aria-hidden className="size-icon-md shrink-0" />
                      <span className="truncate">{names[index]}</span>
                    </button>
                    <TitlebarButton
                      tooltip="Close terminal"
                      shortcut="⌘W"
                      size="icon-xs"
                      className={cn(!selected && "opacity-0 group-hover/tab:opacity-100 focus-visible:opacity-100")}
                      onClick={() => closeTab(place, tab.id)}
                    >
                      <X />
                    </TitlebarButton>
                  </div>
                );
              })}
            </div>
            <TitlebarButton tooltip="New terminal" shortcut="⌘T" onClick={() => addTab(place)}>
              <Plus />
            </TitlebarButton>
            <div className="flex-1" />
            <TitlebarButton tooltip="Hide terminal" shortcut="⌘J" onClick={hide}>
              <ChevronDown />
            </TitlebarButton>
          </header>
        </TitlebarTips>
        {data.tabs.map((tab) => (
          <div
            key={tab.id}
            className={tab.id === data.active ? "flex min-h-0 flex-1 flex-col" : "hidden"}
          >
            <TerminalTab place={place} tabId={tab.id} active={open && tab.id === data.active} />
          </div>
        ))}
      </div>
    </section>
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
  return <TerminalView key={density} open={open} focus={active} />;
}

/**
 * A shell the daemon runs, shown live: `open` starts it (or re-attaches to it) at the view's
 * size. When it ends, `onExit` hears its exit code (a pane's tab just closes).
 */
export function TerminalView({
  open,
  onExit,
  focus = true,
  className,
}: {
  open: (cols: number, rows: number) => Promise<TerminalInfo>;
  onExit?: (code: number | null) => void;
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
    const style = getComputedStyle(element);
    const terminal = new Terminal({
      fontFamily: style.fontFamily,
      fontSize: Number.parseFloat(style.fontSize),
      theme: theme(),
      cursorBlink: true,
      scrollback: 5000,
      linkHandler: {
        activate: (_event, uri) => {
          void openUrl(uri).catch(() => {});
        },
      },
    });
    liveTerminal.current = terminal;
    const fit = new FitAddon();
    terminal.loadAddon(fit);
    terminal.open(element);
    fit.fit();

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
        if (event.code === "KeyC" && terminal.hasSelection()) {
          event.preventDefault();
          void navigator.clipboard
            .writeText(terminal.getSelection())
            .catch(() => {});
          return false;
        }
        if (event.code === "KeyV") {
          event.preventDefault();
          void navigator.clipboard
            .readText()
            .then((text) => terminal.paste(text))
            .catch(() => {});
          return false;
        }
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
      liveTerminal.current = null;
      terminal.dispose();
    };
  }, [open, connected, onExit, mac]);

  return (
    <div
      className={cn("bg-background flex min-h-0 flex-1 flex-col", className)}
    >
      {error && (
        <p role="alert" className="text-destructive shrink-0 px-4 py-2 text-sm">
          {error}
        </p>
      )}
      <div
        ref={host}
        data-slot="terminal"
        className="min-h-0 flex-1 ps-4 pe-2 pt-2 pb-3 font-mono text-xs"
      />
    </div>
  );
}
