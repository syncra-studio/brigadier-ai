import "@xterm/xterm/css/xterm.css";

import { FitAddon } from "@xterm/addon-fit";
import { type ITheme, Terminal } from "@xterm/xterm";
import {
  ChevronDown,
  Plus,
  Terminal as TerminalIcon,
  X,
} from "@openai/apps-sdk-ui/components/Icon";
import { useCallback, useContext, useEffect, useRef, useState } from "react";

import { SidePanelContext, useReveal } from "@/app/conversation/SidePanel";
import { TitlebarButton, TitlebarTips } from "@/components/titlebar-button";
import {
  addTerminalSession,
  describeTerminalSession,
  removeTerminalSession,
  selectTerminalSession,
  terminalSessions,
  undoTerminalClose,
  useTerminalSessions,
} from "@/state/terminalSessions";
import { toast } from "@/state/toasts";

import { openUrl, request } from "@/ipc/client";
import type { TerminalInfo, TerminalOutput } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { useApp } from "@/state/store";
import { noteTerminal, onTerminalOutput } from "@/state/terminals";

/**
 * The Terminal tab (⌃`): the session's shell in its checkout. The shell runs in the
 * daemon and keeps running while the tab is hidden; closing the tab ends it.
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

/** A bottom-only terminal with independent shells. Hiding never ends a shell. */
export function TerminalPane({ conversationId }: { conversationId: string }) {
  const { state, terminalHeight, terminalMaxHeight, resizeTerminal, closeTab } =
    useContext(SidePanelContext);
  const data = useTerminalSessions((s) => s.conversations[conversationId]);
  const start = useRef<{ y: number; height: number } | null>(null);
  const pane = useRef<HTMLElement>(null);
  const previousFocus = useRef<HTMLElement | null>(null);
  const wasOpen = useRef(false);
  useEffect(() => {
    if (state.terminalOpen && !wasOpen.current) {
      previousFocus.current =
        document.activeElement instanceof HTMLElement
          ? document.activeElement
          : null;
    } else if (!state.terminalOpen && wasOpen.current) {
      const editor = document.querySelector<HTMLElement>(
        '[data-slot="composer"] [contenteditable="true"], [data-slot="composer"] textarea',
      );
      (editor ?? previousFocus.current)?.focus();
    }
    wasOpen.current = state.terminalOpen;
  }, [state.terminalOpen]);
  const reveal = useReveal(state.terminalOpen);
  useEffect(() => {
    if (state.terminalOpen && !terminalSessions(conversationId).sessions.length)
      addTerminalSession(conversationId);
  }, [conversationId, state.terminalOpen]);
  const newSession = useCallback(
    () => addTerminalSession(conversationId),
    [conversationId],
  );
  const active = data?.active;
  const close = useCallback(() => {
    if (!active) return;
    removeTerminalSession(conversationId, active);
    if (!terminalSessions(conversationId).sessions.length) closeTab("terminal");
    toast("Terminal closed", {
      actions: [
        {
          label: "Undo",
          run: () => {
            undoTerminalClose(conversationId);
            // A closed process is replaced by a fresh shell; commands are never replayed.
            document.dispatchEvent(
              new CustomEvent("brigadier:restore-terminal", {
                detail: conversationId,
              }),
            );
          },
        },
      ],
    });
  }, [conversationId, active, closeTab]);
  useEffect(() => {
    if (!state.terminalOpen) return;
    const key = (event: KeyboardEvent) => {
      const focused = document.activeElement;
      if (!focused || !pane.current?.contains(focused)) return;
      const command = event.metaKey || event.ctrlKey;
      if (!command || event.altKey) return;
      if (
        event.shiftKey &&
        ["BracketLeft", "BracketRight"].includes(event.code) &&
        data?.sessions.length
      ) {
        event.preventDefault();
        const index = data.sessions.findIndex(
          (session) => session.id === data.active,
        );
        const next =
          (index +
            (event.code === "BracketLeft" ? -1 : 1) +
            data.sessions.length) %
          data.sessions.length;
        selectTerminalSession(conversationId, data.sessions[next]!.id);
        return;
      }
      if (event.code === "KeyT" && !event.shiftKey) {
        event.preventDefault();
        newSession();
      }
      if (event.code === "KeyW" && !event.shiftKey) {
        event.preventDefault();
        close();
      }
    };
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  }, [state.terminalOpen, conversationId, close, newSession, data]);
  if (!reveal.mounted && !data?.sessions.length) return null;
  return (
    <section
      ref={pane}
      data-slot="terminal-pane"
      aria-label="Terminal"
      inert={!state.terminalOpen}
      className={cn(
        "border-border bg-background relative flex min-h-0 shrink-0 flex-col",
        reveal.moving &&
          "ease-panel transition-[height] duration-500 motion-reduce:transition-none",
      )}
      style={{
        height: reveal.out ? terminalHeight : 0,
        visibility: reveal.mounted ? "visible" : "hidden",
      }}
    >
      <div
        role="separator"
        aria-label="Resize terminal"
        aria-orientation="horizontal"
        tabIndex={0}
        aria-valuenow={terminalHeight}
        aria-valuemin={160}
        aria-valuemax={terminalMaxHeight}
        className="group/resize absolute inset-x-0 -top-2 z-10 h-4 cursor-row-resize outline-none"
        onPointerDown={(event) => {
          if (event.button !== 0) return;
          event.preventDefault();
          event.currentTarget.setPointerCapture(event.pointerId);
          start.current = { y: event.clientY, height: terminalHeight };
        }}
        onPointerMove={(event) => {
          if (!start.current) return;
          const height = start.current.height + start.current.y - event.clientY;
          if (height < 90) {
            resizeTerminal(160);
            start.current = null;
            closeTab("terminal");
          } else
            resizeTerminal(Math.max(160, Math.min(height, terminalMaxHeight)));
        }}
        onPointerUp={(event) => {
          start.current = null;
          event.currentTarget.releasePointerCapture(event.pointerId);
        }}
        onPointerCancel={() => {
          start.current = null;
        }}
        onDoubleClick={() => resizeTerminal(null)}
        onKeyDown={(event) => {
          const value =
            event.key === "ArrowUp"
              ? terminalHeight + 16
              : event.key === "ArrowDown"
                ? terminalHeight - 16
                : event.key === "Home"
                  ? 160
                  : event.key === "End"
                    ? terminalMaxHeight
                    : null;
          if (value === null) return;
          event.preventDefault();
          resizeTerminal(Math.max(160, Math.min(value, terminalMaxHeight)));
        }}
      >
        <span className="bg-input absolute inset-x-0 top-1 h-px opacity-0 group-hover/resize:opacity-100 group-focus-visible/resize:opacity-100" />
      </div>
      <div className="border-border flex h-full min-h-0 flex-col overflow-hidden border-t">
        <header className="flex h-10 shrink-0 items-center gap-2 px-3">
          <TerminalIcon
            aria-hidden
            className="size-icon-md text-toolbar-foreground shrink-0"
          />
          <select
            aria-label="Terminal session"
            value={active ?? ""}
            onChange={(event) =>
              selectTerminalSession(conversationId, event.target.value)
            }
            className="bg-panel-tab shadow-panel-tab rounded-lg min-w-0 max-w-64 px-2 py-1 text-sm outline-none focus-visible:ring-1 focus-visible:ring-ring"
          >
            {data?.sessions.map((session, index) => (
              <option key={session.id} value={session.id}>
                {session.title}
                {data.sessions.length > 1 ? ` ${index + 1}` : ""}
              </option>
            ))}
          </select>
          <TitlebarTips>
            <TitlebarButton
              tooltip="Close terminal session"
              shortcut="⌘W"
              onClick={close}
            >
              <X />
            </TitlebarButton>
            <TitlebarButton
              tooltip="New terminal"
              shortcut="⌘T"
              onClick={newSession}
            >
              <Plus />
            </TitlebarButton>
            <div className="flex-1" />
            <TitlebarButton
              tooltip="Hide terminal"
              shortcut="⌃`"
              onClick={() => closeTab("terminal")}
            >
              <ChevronDown />
            </TitlebarButton>
          </TitlebarTips>
        </header>
        {data?.sessions.map((session) => (
          <div
            key={session.id}
            className={
              session.id === active ? "flex min-h-0 flex-1 flex-col" : "hidden"
            }
          >
            <TerminalTab
              conversationId={conversationId}
              sessionId={session.id}
              active={state.terminalOpen && session.id === active}
            />
          </div>
        ))}
      </div>
    </section>
  );
}

export function TerminalTab({
  conversationId,
  sessionId,
  active,
}: {
  conversationId: string;
  sessionId: string;
  active: boolean;
}) {
  const density = useApp((s) => s.settings.density);
  const [generation, setGeneration] = useState(0);
  const restart = useCallback(
    () => setGeneration((current) => current + 1),
    [],
  );
  const open = useCallback(
    async (cols: number, rows: number) => {
      const { terminal } = await request({
        method: "openTerminal",
        conversationId,
        sessionId,
        cols,
        rows,
      });
      if (
        !terminalSessions(conversationId).sessions.some(
          (session) => session.id === sessionId,
        )
      ) {
        void request({
          method: "closeTerminal",
          terminalId: terminal.id,
        }).catch(() => {});
        throw new Error("Terminal closed");
      }
      noteTerminal(sessionId, terminal.id);
      describeTerminalSession(conversationId, sessionId, terminal.cwd);
      return terminal;
    },
    [conversationId, sessionId],
  );
  return (
    <TerminalView
      key={`${density}:${generation}`}
      open={open}
      onRestart={restart}
      focus={active}
    />
  );
}

/**
 * A shell the daemon runs, shown live: `open` starts it (or re-attaches to it) at the view's
 * size. When it ends, `onExit` hears its exit code; without one, the next key pressed calls
 * `onRestart` for a new shell.
 */
export function TerminalView({
  open,
  onRestart,
  onExit,
  focus = true,
  className,
}: {
  open: (cols: number, rows: number) => Promise<TerminalInfo>;
  onRestart?: () => void;
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
      const code = output.code === null ? "" : ` with code ${output.code}`;
      if (onExit) {
        terminal.write(`\r\n${DIM}[Process exited${code}.]${RESET}\r\n`);
        onExit(output.code);
        return;
      }
      terminal.write(
        `\r\n${DIM}[Process exited${code}. Press any key to start a new shell.]${RESET}\r\n`,
      );
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
      if (ended) {
        onRestart?.();
        return;
      }
      if (id) {
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
  }, [open, connected, onRestart, onExit, mac]);

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
