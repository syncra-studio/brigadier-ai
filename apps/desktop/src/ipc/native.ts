// The one door from the app's code to its native shell. Everything else in src/ imports from here, or
// from `nativeMocks` in tests and fixtures, and never from `@tauri-apps/*` (.oxlintrc.json enforces it).
//
// Two shells stand behind it while the app moves to Electron. Which one is decided at each call, since
// tests and fixtures install their stand-in after this module loads: Electron's preload exposes
// `window.brigadierNative`, and anywhere else is Tauri or its mocks.
import { Channel as TauriChannel, invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen as tauriListen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";

export type UnlistenFn = () => void;

/** A stream of messages from the shell, passed as an argument to the command that feeds it. */
export type Channel<T> = { onmessage: (message: T) => void };

/** What a window's drop handler hears; `position` is in the page's pixels. */
export type DragDropPayload =
  | { type: "enter"; paths: string[]; position: { x: number; y: number } }
  | { type: "over"; position: { x: number; y: number } }
  | { type: "drop"; paths: string[]; position: { x: number; y: number } }
  | { type: "leave" };

/** Electron's side of the door, from apps/desktop/electron's preload. */
type ElectronNative = {
  /** An envelope, since a rejection across Electron's IPC keeps only a message. `json` is left unread by main. */
  invoke(command: string, args?: unknown): Promise<{ json: string } | { value: unknown } | { error: unknown }>;
  listen(event: string, handler: (payload: unknown) => void): UnlistenFn;
  onChannel(handler: (id: number, json: string) => void): void;
  onResized(handler: () => void): UnlistenFn;
  /** The real path of a dropped file; only the preload can read it. */
  pathForFile(file: File): string;
};

declare global {
  interface Window {
    brigadierNative?: ElectronNative;
  }
}

function electron(): ElectronNative | undefined {
  return typeof window === "undefined" ? undefined : window.brigadierNative;
}

// A channel crosses Electron's preload as its id: main sends to the id, and this page finds the listener.
const electronChannels = new Map<number, Channel<never>>();
const electronChannelIds = new WeakMap<object, number>();
let nextElectronChannel = 1;
let hearingElectronChannels = false;

/** Makes a channel for the shell in charge, calling `onMessage` with each of its messages. */
export function newChannel<T>(onMessage: (message: T) => void): Channel<T> {
  const shell = electron();
  if (!shell) return new TauriChannel<T>(onMessage);
  if (!hearingElectronChannels) {
    hearingElectronChannels = true;
    shell.onChannel((id, json) => electronChannels.get(id)?.onmessage(JSON.parse(json) as never));
  }
  const channel: Channel<T> = { onmessage: onMessage };
  const id = nextElectronChannel++;
  electronChannels.set(id, channel as Channel<never>);
  electronChannelIds.set(channel, id);
  return channel;
}

/** The arguments as they can cross to Electron's main: a channel goes as its id. */
function electronArgs(args: Record<string, unknown> | undefined): Record<string, unknown> | undefined {
  if (!args) return args;
  return Object.fromEntries(
    Object.entries(args).map(([name, value]) => {
      const id = typeof value === "object" && value !== null ? electronChannelIds.get(value) : undefined;
      return [name, id === undefined ? value : { channelId: id }];
    }),
  );
}

/** Runs a command in the shell and returns its answer; the shell's error is the rejection. */
export function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const shell = electron();
  if (!shell) return tauriInvoke<T>(command, args);
  const inPage = pageCommands?.[command];
  if (inPage) return Promise.resolve(inPage(args ?? {}) as T);
  return shell.invoke(command, electronArgs(args)).then((answer) => {
    if ("error" in answer) throw answer.error;
    return ("json" in answer ? JSON.parse(answer.json) : answer.value) as T;
  });
}

type PageCommand = (args: Record<string, unknown>) => unknown;
let pageCommands: Record<string, PageCommand> | null = null;

/** Commands the page answers itself under Electron: a tab's `<webview>` lives in the page. */
export function setPageCommands(commands: Record<string, PageCommand>): void {
  pageCommands = commands;
}

/** Hears an event the shell sends this window. */
export function listen<T>(event: string, handler: (event: { payload: T }) => void): Promise<UnlistenFn> {
  const shell = electron();
  if (shell) return Promise.resolve(shell.listen(event, (payload) => handler({ payload: payload as T })));
  return tauriListen<T>(event, handler);
}

/** This window's own state and its closing. */
export const nativeWindow = {
  isFullscreen(): Promise<boolean> {
    return electron() ? invoke<boolean>("is_fullscreen") : getCurrentWindow().isFullscreen();
  },
  onResized(handler: () => void): Promise<UnlistenFn> {
    const shell = electron();
    return shell ? Promise.resolve(shell.onResized(handler)) : getCurrentWindow().onResized(handler);
  },
  close(): Promise<void> {
    return electron() ? invoke<void>("close_window") : getCurrentWindow().close();
  },
};

const carriesFiles = (event: DragEvent) => event.dataTransfer?.types.includes("Files") ?? false;
const position = (event: DragEvent) => ({ x: event.clientX, y: event.clientY });

/** Files dragged over the page in Electron, where the page itself sees the drag and only the preload can read paths. */
function electronDragDrop(shell: ElectronNative, handler: (payload: DragDropPayload) => void): UnlistenFn {
  const paths = (event: DragEvent) =>
    [...(event.dataTransfer?.files ?? [])].map((file) => shell.pathForFile(file)).filter((path) => path !== "");
  // Enter and leave fire for each element the drag crosses: a leave counts only once it has left them all.
  let depth = 0;
  const enter = (event: DragEvent) => {
    if (!carriesFiles(event)) return;
    event.preventDefault();
    depth += 1;
    if (depth === 1) handler({ type: "enter", paths: paths(event), position: position(event) });
  };
  const over = (event: DragEvent) => {
    if (!carriesFiles(event)) return;
    event.preventDefault();
    handler({ type: "over", position: position(event) });
  };
  const leave = (event: DragEvent) => {
    if (!carriesFiles(event)) return;
    depth = Math.max(0, depth - 1);
    if (depth === 0) handler({ type: "leave" });
  };
  const drop = (event: DragEvent) => {
    if (!carriesFiles(event)) return;
    event.preventDefault();
    depth = 0;
    handler({ type: "drop", paths: paths(event), position: position(event) });
  };
  window.addEventListener("dragenter", enter);
  window.addEventListener("dragover", over);
  window.addEventListener("dragleave", leave);
  window.addEventListener("drop", drop);
  return () => {
    window.removeEventListener("dragenter", enter);
    window.removeEventListener("dragover", over);
    window.removeEventListener("dragleave", leave);
    window.removeEventListener("drop", drop);
  };
}

/** Under Electron `onDragDrop` hears every drop with its real paths, so nothing else in the page should take one. */
export function dropsComeByWindow(): boolean {
  return Boolean(electron());
}

/** Hears files dragged over and dropped on this window. */
export function onDragDrop(handler: (payload: DragDropPayload) => void): Promise<UnlistenFn> {
  const shell = electron();
  if (shell) return Promise.resolve(electronDragDrop(shell, handler));
  return getCurrentWebview().onDragDropEvent(({ payload }) => handler(payload));
}
