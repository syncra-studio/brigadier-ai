import { Channel, invoke } from "@tauri-apps/api/core";

import type {
  AppInfo,
  BridgeEvent,
  BrowserBounds,
  BrowserEvent,
  ConventionsExport,
  IpcError,
  RunningChat,
  Request,
  Response,
  SmokeReport,
  UiMeasurements,
} from "@/ipc/generated";

export type Method = Request["method"];
export type RequestOf<M extends Method> = Extract<Request, { method: M }>;
export type ResponseOf<M extends Method> = Extract<Response, { method: M }>;

/** Rejection raised by `request` when the daemon (or the bridge) reports an error. */
export class RequestError extends Error {
  readonly code: IpcError["code"];

  constructor(error: IpcError) {
    super(error.message);
    this.name = "RequestError";
    this.code = error.code;
  }
}

function isIpcError(value: unknown): value is IpcError {
  return (
    typeof value === "object" &&
    value !== null &&
    "code" in value &&
    "message" in value
  );
}

/** Sends a typed request to brigadierd through the shell and returns its paired response. */
export async function request<M extends Method>(
  req: RequestOf<M>,
): Promise<ResponseOf<M>> {
  try {
    const response = await invoke<Response>("ipc_request", { request: req });
    if (response.method !== req.method) {
      throw new Error(
        `expected a ${req.method} response, got ${response.method}`,
      );
    }
    return response as ResponseOf<M>;
  } catch (error) {
    throw isIpcError(error) ? new RequestError(error) : error;
  }
}

/** Routes the shell's bridge events (daemon events, metrics, connection state) to `onEvent`. */
export async function subscribe(
  onEvent: (event: BridgeEvent) => void,
): Promise<void> {
  const channel = new Channel<BridgeEvent>(onEvent);
  await invoke("ipc_subscribe", { channel });
}

export function appInfo(): Promise<AppInfo> {
  return invoke<AppInfo>("app_info");
}

/** Reports when the app became usable; resolves to cold start in ms. */
export function appReady(readyMs: number): Promise<number> {
  return invoke<number>("app_ready", { readyMs });
}

/** The startup screen has gone: the window drops its startup backdrop. */
export function startupFinished(): Promise<void> {
  return invoke("startup_finished");
}

export function smokeFinish(
  measurements: UiMeasurements,
): Promise<SmokeReport> {
  return invoke<SmokeReport>("smoke_finish", { measurements });
}

/** High-resolution wall clock in ms since the Unix epoch, comparable with daemon timestamps. */
export function nowEpochMs(): number {
  return performance.timeOrigin + performance.now();
}

/** Opens the system folder picker (at `starting` when it exists); `null` when cancelled. */
export function pickFolder(starting?: string): Promise<string | null> {
  return invoke<string | null>("pick_folder", { starting: starting || null });
}

/** File › Open Folder…'s picker: folders to add as projects, none when cancelled. */
export function pickFolders(): Promise<string[]> {
  return invoke<string[]>("pick_folders");
}

/** The folders opened with the app (Finder, the Dock, launch arguments) not yet taken. */
export function takeOpenedFolders(): Promise<string[]> {
  return invoke<string[]>("take_opened_folders");
}

/** Saves an artifact where the user picks in the system save dialog; `false` when cancelled. */
export async function saveArtifact(id: string, fileName: string): Promise<boolean> {
  try {
    return await invoke<boolean>("save_artifact", { id, fileName });
  } catch (error) {
    throw isIpcError(error) ? new RequestError(error) : error;
  }
}

/**
 * Exports a project's conventions to the AGENTS.md picked in the system save dialog, which
 * starts in `directory`; `null` when cancelled. Scripts use the `exportConventions` request.
 */
export async function exportConventions(
  projectId: string,
  directory: string | null,
): Promise<ConventionsExport | null> {
  try {
    return await invoke<ConventionsExport | null>("export_conventions", { projectId, directory });
  } catch (error) {
    throw isIpcError(error) ? new RequestError(error) : error;
  }
}

/** Opens an artifact with the system's default app for its type. */
export async function openArtifact(id: string, fileName: string): Promise<void> {
  try {
    await invoke("open_artifact", { id, fileName });
  } catch (error) {
    throw isIpcError(error) ? new RequestError(error) : error;
  }
}

/** Opens a web link (http or https) in the user's browser. */
export async function openUrl(url: string): Promise<void> {
  try {
    await invoke("open_url", { url });
  } catch (error) {
    throw isIpcError(error) ? new RequestError(error) : error;
  }
}

/** The conversations the menu-bar item lists under "Running". */
export function setRunningChats(chats: RunningChat[]): Promise<void> {
  return invoke("set_running_chats", { chats });
}

/** Shows a file selected in the system file manager. */
export async function revealPath(path: string): Promise<void> {
  try {
    await invoke("reveal_path", { path });
  } catch (error) {
    throw isIpcError(error) ? new RequestError(error) : error;
  }
}

/** Opens a folder in the system file manager, or with the app named `app` (macOS). */
export async function openFolder(path: string, app?: string): Promise<void> {
  try {
    await invoke("open_folder", { path, with: app ?? null });
  } catch (error) {
    throw isIpcError(error) ? new RequestError(error) : error;
  }
}

/**
 * Makes the Browser tab `id`'s page, showing `url` over `bounds`; `onEvent` hears that page
 * until it is closed.
 */
export async function browserOpen(
  id: string,
  url: string,
  bounds: BrowserBounds,
  onEvent: (event: BrowserEvent) => void,
): Promise<void> {
  try {
    await invoke("browser_open", { id, url, bounds, events: new Channel<BrowserEvent>(onEvent) });
  } catch (error) {
    throw isIpcError(error) ? new RequestError(error) : error;
  }
}

/** Shows `url` in the Browser tab `id`'s page, over `bounds`. */
export async function browserNavigate(
  id: string,
  url: string,
  bounds: BrowserBounds,
): Promise<void> {
  try {
    await invoke("browser_navigate", { id, url, bounds });
  } catch (error) {
    throw isIpcError(error) ? new RequestError(error) : error;
  }
}

/** Moves the Browser tab's page to `bounds`, or hides it (`null`). */
export function browserPlace(id: string, bounds: BrowserBounds | null): Promise<void> {
  return invoke("browser_place", { id, bounds });
}

/** Back, forward, reload or stop in the Browser tab's page. */
export function browserGo(
  id: string,
  action: "back" | "forward" | "reload" | "stop",
): Promise<void> {
  return invoke("browser_go", { id, action });
}

/** Drops the Browser tab's page and all it stored. */
export function browserClose(id: string): Promise<void> {
  return invoke("browser_close", { id });
}

/** Whether Brigadier may show notifications, as the OS says without asking. */
export type NotificationPermission = "unknown" | "notAsked" | "off" | "allowed";

export function notificationPermission(): Promise<NotificationPermission> {
  return invoke<NotificationPermission>("notification_permission");
}

/** Opens the system's notification settings at Brigadier. */
export function openNotificationSettings(): Promise<void> {
  return invoke<void>("open_notification_settings");
}

/** Rust chooses and remembers the destination; this call never accepts a write path. `choose`
 * asks for a destination even when one is remembered (Save As). */
export function saveDocument(id: string, text: string, directory: string | null, name: string, choose = false): Promise<string | null> {
  return invoke("save_document", { id, text, directory, name, choose });
}

/** Release the native save grant when a document leaves the reopen history. */
export function forgetDocument(id: string): Promise<void> {
  return invoke("forget_document", { id });
}
