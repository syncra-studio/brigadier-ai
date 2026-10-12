import { Channel, invoke } from "@tauri-apps/api/core";

import type {
  AppInfo,
  BridgeEvent,
  BrowserBounds,
  BrowserEvent,
  BrowserFound,
  BrowserOptions,
  ConventionsExport,
  IpcError,
  Page,
  PageContent,
  PageSummary,
  PageWriteOutcome,
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
 * until it is closed. `agent`: the tab came from something an agent gave (its history says so).
 */
export async function browserOpen(
  id: string,
  url: string,
  bounds: BrowserBounds,
  agent: boolean,
  onEvent: (event: BrowserEvent) => void,
): Promise<void> {
  try {
    await invoke("browser_open", { id, url, bounds, agent, events: new Channel<BrowserEvent>(onEvent) });
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

/**
 * Back, forward, reload, reload past the cache (macOS) or stop in the Browser tab's page. The
 * `-unless-typing` ones are ⌘← and ⌘→ while the page has the keyboard: in a text field they move
 * the caret to the line's start or end instead.
 */
export function browserGo(
  id: string,
  action: "back" | "forward" | "back-unless-typing" | "forward-unless-typing" | "reload" | "force-reload" | "stop",
): Promise<void> {
  return invoke("browser_go", { id, action });
}

/** The Browser tab's page zoom (1 is 100%), set to `zoom` first when given (macOS). */
export function browserZoom(id: string, zoom: number | null): Promise<number> {
  return invoke<number>("browser_zoom", { id, zoom });
}

/**
 * Finds `query` in the Browser tab's page (macOS): `"type"` from where the match selected
 * starts (the query changed), `"next"` and `"previous"` from it on. An empty query leaves no
 * match selected.
 */
export function browserFind(id: string, query: string, step: "type" | "next" | "previous"): Promise<BrowserFound> {
  return invoke<BrowserFound>("browser_find", { id, query, step });
}

/** Opens the print panel for the Browser tab's page (macOS). */
export function browserPrint(id: string): Promise<void> {
  return invoke("browser_print", { id });
}

/** Puts a picture of the Browser tab's page on the clipboard (macOS). */
export function browserSnapshot(id: string): Promise<void> {
  return invoke("browser_snapshot", { id });
}

/** Gives the keyboard to the Browser tab `id`'s page, or with `null` back to the app's own. */
export function browserFocus(id: string | null): Promise<void> {
  return invoke("browser_focus", { id });
}

/** Tells the shell the user's Settings › Browser choices it acts on itself. */
export function browserConfigure(options: BrowserOptions): Promise<void> {
  return invoke("browser_configure", { options });
}

/** Drops the Browser tab's page (what it saved stays in the browser's store). */
export function browserClose(id: string): Promise<void> {
  return invoke("browser_close", { id });
}

/** Whether Brigadier may show notifications, as the OS says without asking. */
export type NotificationPermission = "unknown" | "notAsked" | "off" | "allowed";

export function notificationPermission(): Promise<NotificationPermission> {
  return invoke<NotificationPermission>("notification_permission");
}

/** Asks the OS to let Brigadier notify, and answers the user's choice once made. */
export function askNotificationPermission(): Promise<NotificationPermission> {
  return invoke<NotificationPermission>("ask_notification_permission");
}

/** Opens the system's notification settings at Brigadier. */
export function openNotificationSettings(): Promise<void> {
  return invoke<void>("open_notification_settings");
}

/** Whether Brigadier opens at login, as the system says (not where it has no way, or a
 * development build on macOS). */
export type LoginItem = "unsupported" | "off" | "on" | "needsApproval";

export function loginItem(): Promise<LoginItem> {
  return invoke<LoginItem>("login_item");
}

/** Turns opening at login on or off, and answers how it stands then. */
export function setLoginItem(on: boolean): Promise<LoginItem> {
  return invoke<LoginItem>("set_login_item", { on });
}

/** Opens the system's login items, where macOS asks the user to allow Brigadier. */
export function openLoginItems(): Promise<void> {
  return invoke<void>("open_login_items");
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

/** Asks where to save a copy of `text` (suggesting `name`) and writes it there; the path, or
 * null when the dialog was cancelled. Nothing is remembered for the next save. */
export async function saveCopy(text: string, name: string): Promise<string | null> {
  const id = `copy:${crypto.randomUUID()}`;
  try {
    return await saveDocument(id, text, null, name, true);
  } finally {
    forgetDocument(id).catch(console.error);
  }
}

/** Makes a new, empty page: a local Markdown document the daemon keeps in the data folder. */
export async function pageCreate(): Promise<Page> {
  return (await request({ method: "pageCreate" })).page;
}

/** A page as it is on disk now; its `revision` is what the next `pageWrite` is based on. */
export async function pageRead(id: string): Promise<Page> {
  return (await request({ method: "pageRead", id })).page;
}

/** Saves a page if it is still at `baseRevision`; otherwise nothing is written and the outcome
 * is a conflict carrying the page as it is now. */
export async function pageWrite(
  id: string,
  content: PageContent,
  baseRevision: string,
): Promise<PageWriteOutcome> {
  return (await request({ method: "pageWrite", id, content, baseRevision })).outcome;
}

/** Every page, the most recently changed first. */
export async function pageList(): Promise<PageSummary[]> {
  return (await request({ method: "pageList" })).pages;
}

/** Deletes a page; one already gone is fine. */
export async function pageDelete(id: string): Promise<void> {
  await request({ method: "pageDelete", id });
}

/** Answers a quit's `flush-pages` request: the pages' pending edits are saved. */
export function pagesFlushed(token: number): Promise<void> {
  return invoke("pages_flushed", { token });
}
