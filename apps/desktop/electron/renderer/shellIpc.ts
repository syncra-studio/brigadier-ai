// What the app asks of its shell, answered in the page. `browser_*` reach the tab layer; the rest is
// a stand-in for the daemon, here only so the real front end runs while stage 1 proves the browser.
// When the app moves to Electron (stage 2) these commands get real implementations.
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import type { BrowserBounds, BrowserEvent } from "@/ipc/generated";

import { answerDaemon, startDaemonEvents } from "./devDaemon";
import { closePage, find, go, navigatePage, openPage, placePage, printPage, setModal, zoom } from "./tabLayer";

type Args = Record<string, unknown>;
type Channelish = { onmessage: (message: unknown) => void };

const unknown = new Map<string, number>();
const listeners = new Map<number, string>();

/** Commands nothing implements yet, for the check that says which the shell still lacks. */
export function unhandledCommands(): Record<string, number> {
  return Object.fromEntries(unknown);
}

function answer(command: string, args: Args): unknown {
  switch (command) {
    case "browser_open": {
      const events = args.events as Channelish;
      return openPage(args.id as string, args.url as string, args.bounds as BrowserBounds, (event: BrowserEvent) => events.onmessage(event));
    }
    case "browser_navigate":
      return navigatePage(args.id as string, args.url as string, args.bounds as BrowserBounds);
    case "browser_place":
      return placePage(args.id as string, args.bounds as BrowserBounds | null);
    case "browser_layer":
      return setModal(args.modal as boolean);
    case "browser_go":
      return go(args.id as string, args.action as string);
    case "browser_zoom":
      return zoom(args.id as string, args.zoom as number | null);
    case "browser_find":
      return find(args.id as string, args.query as string, args.step as "type" | "next" | "previous");
    case "browser_print":
      return printPage(args.id as string);
    case "browser_close":
      return closePage(args.id as string);
    case "browser_configure":
    case "browser_focus":
    case "browser_permission":
      return null;
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
