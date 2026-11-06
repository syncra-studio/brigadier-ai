// Dev only: what the driver asks of the main process, reached from the hidden control page over the
// debugging pipe. A fixed list, so a check can't quietly do something the app itself can't.
import { app, ipcMain, screen, session, webContents, type BrowserWindow, type WebContents } from "electron";

import type { Note } from "./events";

type Args = Record<string, never> & {
  name: string;
  bounds: Electron.Rectangle;
  query: string;
  real: boolean;
  level: Parameters<BrowserWindow["setAlwaysOnTop"]>[1] | "normal";
  on: boolean;
  id: number;
  event: Electron.MouseInputEvent | Electron.KeyboardInputEvent;
  text: string;
  js: string;
  stayHidden: boolean;
  point: { x: number; y: number };
  partition: string;
};

type Control = {
  createWindow: (options: { name: string; bounds?: Partial<Electron.Rectangle>; query?: string; real?: boolean }) => BrowserWindow;
  windows: Map<string, BrowserWindow>;
  note: Note;
};

const guest = (id: number): WebContents => {
  const found = webContents.fromId(id);
  if (!found) throw new Error(`No guest ${id}`);
  return found;
};

export function installDevControl({ createWindow, windows, note }: Control): void {
  /** Cursors each guest asked for, so a check can read what a real pointer would have shown. */
  const cursors = new Map<number, string[]>();
  app.on("web-contents-created", (_event, contents) => {
    if (contents.getType() === "webview") contents.on("cursor-changed", (_change, type) => cursors.set(contents.id, [...(cursors.get(contents.id) ?? []), type].slice(-20)));
  });
  const parked = new Map<BrowserWindow, Electron.Rectangle>();
  const need = (name: string): BrowserWindow => {
    const window = windows.get(name);
    if (!window) throw new Error(`No window "${name}"`);
    return window;
  };
  const ops: Record<string, (args: Args) => unknown> = {
    info: () => ({
      pid: process.pid,
      electron: process.versions.electron,
      chrome: process.versions.chrome,
      windows: [...windows.keys()],
      display: screen.getPrimaryDisplay().bounds,
      workArea: screen.getPrimaryDisplay().workArea,
    }),
    // Dev windows start hidden and small; openShell parks them before they ever show.
    createWindow: ({ name, bounds, query, real }) => {
      const window = createWindow({ name, bounds, query, real });
      return new Promise((resolve) => window.webContents.once("did-finish-load", () => resolve({ name, id: window.id, wc: window.webContents.id, media: window.getMediaSourceId() })));
    },
    // Off the screen first, shown, then the strip: at no moment is the window over the user's work.
    showAtEdge: ({ name, bounds }) => {
      const window = need(name);
      window.setBounds(bounds);
      window.setOpacity(0);
      window.showInactive();
      window.setBounds(bounds);
      window.setOpacity(1);
      return window.getBounds();
    },
    // Why a window isn't on screen, and what puts it there without activating the app.
    windowDiag: ({ name }) => {
      const window = need(name);
      return { visible: window.isVisible(), minimized: window.isMinimized(), opacity: window.getOpacity(), appHidden: app.isHidden(), appActive: app.isActive?.() ?? null, onTop: window.isAlwaysOnTop() };
    },
    showInactiveAgain: ({ name }) => { need(name).showInactive(); return true; },
    moveTop: ({ name }) => { need(name).moveTop(); return true; },
    ignoreMouse: ({ name, on }) => { need(name).setIgnoreMouseEvents(on); return true; },
    windowInfo: ({ name }) => {
      const window = need(name);
      return {
        media: window.getMediaSourceId(),
        bounds: window.getBounds(),
        visible: window.isVisible(),
        minimized: window.isMinimized(),
        focused: window.isFocused(),
        wc: window.webContents.id,
      };
    },
    setBounds: ({ name, bounds }) => { need(name).setBounds(bounds); return need(name).getBounds(); },
    setLevel: ({ name, level }) => {
      const window = need(name);
      if (level === "normal") window.setAlwaysOnTop(false);
      else window.setAlwaysOnTop(true, level);
      return true;
    },
    allSpaces: ({ name, on }) => {
      need(name).setVisibleOnAllWorkspaces(on, { visibleOnFullScreen: on, skipTransformProcessType: true });
      return true;
    },
    minimize: ({ name }) => { need(name).minimize(); return true; },
    restore: ({ name }) => { need(name).restore(); return true; },
    closeWindow: ({ name }) => { need(name).destroy(); return true; },
    hideApp: () => {
      for (const window of windows.values()) if (window.isVisible()) parked.set(window, window.getBounds());
      app.hide();
      return true;
    },
    // macOS puts an un-hidden window back on the screen: it comes back unseen and goes to its place.
    showApp: () => {
      for (const window of parked.keys()) window.setOpacity(0);
      app.show();
      for (const [window, bounds] of parked) window.setBounds(bounds);
      setTimeout(() => {
        for (const [window, bounds] of parked) {
          window.setBounds(bounds);
          window.setOpacity(1);
        }
        parked.clear();
      }, 300);
      return true;
    },
    metrics: () => app.getAppMetrics().map((m) => ({ pid: m.pid, type: m.type, kb: m.memory?.workingSetSize, sandboxed: m.sandboxed, service: m.serviceName })),
    guests: () => webContents.getAllWebContents().filter((c) => c.getType() === "webview").map((c) => ({
      id: c.id,
      pid: c.getOSProcessId(),
      url: c.getURL(),
      hostWc: c.hostWebContents?.id,
      partition: c.session.storagePath,
      crashed: c.isCrashed(),
      capturing: c.isBeingCaptured(),
    })),
    hostCapturing: ({ name }) => need(name).webContents.isBeingCaptured(),
    // For the input half of checks: scripted and labelled so, never a real pointer.
    guestSendInput: ({ id, event }) => { guest(id).sendInputEvent(event); return true; },
    guestInsertText: ({ id, text }) => guest(id).insertText(text),
    hostSendInput: ({ name, event }) => { need(name).webContents.sendInputEvent(event); return true; },
    hostInsertText: ({ name, text }) => need(name).webContents.insertText(text),
    guestCursors: ({ id }) => cursors.get(id) ?? [],
    guestExec: ({ id, js }) => guest(id).executeJavaScript(js, false),
    // Only the spike's blank-state reproduction uses this: checks read OS pixels, not capturePage.
    capturePageForRepro: async ({ name, stayHidden, point }) => {
      const window = need(name);
      const image = await window.webContents.capturePage(undefined, { stayHidden: Boolean(stayHidden) });
      const size = image.getSize();
      const scale = size.width / window.getContentBounds().width;
      const bitmap = image.toBitmap();
      const at = (Math.round(point.y * scale) * size.width + Math.round(point.x * scale)) * 4;
      return { size, pixel: [bitmap[at + 2], bitmap[at + 1], bitmap[at]] };
    },
    cookies: async ({ partition }) => {
      const list = await session.fromPartition(partition).cookies.get({});
      return list.map((c) => ({ name: c.name, domain: c.domain, session: c.session, expires: c.expirationDate ?? null, hostOnly: c.hostOnly, httpOnly: c.httpOnly, secure: c.secure, sameSite: c.sameSite }));
    },
    flushCookies: async ({ partition }) => {
      await session.fromPartition(partition).cookies.flushStore();
      return true;
    },
    quit: () => { setTimeout(() => app.quit(), 50); return true; },
    kill: () => { setTimeout(() => process.kill(process.pid, "SIGKILL"), 50); return true; },
  };
  ipcMain.handle("brigadier:dev-control", (_event, op: string, args?: Args) => {
    const run = ops[op];
    if (!run) throw new Error(`Unknown op ${op}`);
    note("dev-control", { op, name: args?.name });
    return run(args ?? ({} as Args));
  });
}
