import "@xterm/xterm/css/xterm.css";

import { ClipboardAddon } from "@xterm/addon-clipboard";
import { FitAddon } from "@xterm/addon-fit";
import { SerializeAddon } from "@xterm/addon-serialize";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { type ITheme, Terminal } from "@xterm/xterm";
import { useEffect, useRef, useState } from "react";

import { openUrl, request } from "@/ipc/client";
import type { TerminalInfo, TerminalOutput } from "@/ipc/generated";
import { tokenColor } from "@/lib/tokens";
import { cn } from "@/lib/utils";
import { useApp } from "@/state/store";
import { onTerminalOutput } from "@/state/terminalPlaces";
import { localTerminalFolder } from "@/state/terminalPaths";

/*
 * The live terminal (xterm), apart from the pane so it loads only when a shell first shows.
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

/**
 * A shell the daemon runs, shown live: `open` starts it (or re-attaches to it) at the view's
 * size. When it ends, `onExit` hears its exit code (a pane's tab just closes). `restored` gives
 * output to show above the shell's, `onClear` hears ⌘K, `onTitle` hears the titles the shell
 * sets, and `reader` learns how to read what the view shows, until the function it returns is
 * called.
 */
export function TerminalView({
  open,
  onExit,
  restored,
  onClear,
  onTitle,
  onCwd,
  reader,
  focus = true,
  className,
}: {
  open: (cols: number, rows: number) => Promise<TerminalInfo>;
  onExit?: (code: number | null) => void;
  restored?: () => string | null;
  onClear?: () => void;
  onTitle?: (title: string) => void;
  onCwd?: (cwd: string) => void;
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
    const before = restored?.();
    if (before) terminal.write(`${before}${RESET}\r\n`);
    const title = onTitle ? terminal.onTitleChange(onTitle) : null;
    const cwd = onCwd ? terminal.parser.registerOscHandler(7, (value) => {
      const path = localTerminalFolder(value);
      if (path) onCwd(path);
      return false;
    }) : null;
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
        onClear?.();
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
      cwd?.dispose();
      unread?.();
      liveTerminal.current = null;
      terminal.dispose();
    };
  }, [open, connected, onExit, mac, restored, onClear, onTitle, onCwd, reader]);

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
