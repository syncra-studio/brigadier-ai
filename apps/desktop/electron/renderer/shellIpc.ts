// A dev run's stand-in windows: the shell and the daemon answered in the page, so the stage 1 browser
// checks keep their own sessions per window. The real window goes through `@/ipc/native`.
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";

import { browserCommands } from "./browserCommands";
import { answerDaemon, startDaemonEvents } from "./devDaemon";

type Args = Record<string, unknown>;
type Channelish = { onmessage: (message: unknown) => void };

const unknown = new Map<string, number>();
const listeners = new Map<number, string>();

/** Commands nothing implements yet, for the check that says which the shell still lacks. */
export function unhandledCommands(): Record<string, number> {
  return Object.fromEntries(unknown);
}

function answer(command: string, args: Args): unknown {
  const inPage = browserCommands[command];
  if (inPage) return inPage(args);
  switch (command) {
    case "plugin:event|listen":
      listeners.set(args.handler as number, args.event as string);
      return args.handler;
    case "plugin:event|unlisten":
      listeners.delete(args.eventId as number);
      return null;
    case "ipc_request":
      return answerDaemon(args.request as { method: string });
    case "ipc_subscribe":
      startDaemonEvents(args.channel as Channelish);
      return null;
    case "app_info":
      return { version: "0.0.0-electron-dev", platform: "macos", processStartMs: Date.now(), smoke: false, firstLaunch: false };
    case "app_ready":
      return 0;
    case "now_epoch_ms":
      return Date.now();
    case "app_update":
      return null;
    case "take_opened_folders":
      return [];
    default:
      unknown.set(command, (unknown.get(command) ?? 0) + 1);
      return null;
  }
}

export function installShellIpc(): void {
  mockWindows("main");
  mockIPC((command, payload) => answer(command, (payload ?? {}) as Args));
}
