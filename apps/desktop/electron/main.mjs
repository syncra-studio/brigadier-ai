// Brigadier's Electron main process. Stage 1: one window type, the real front end, the browser's
// security foundations. See tools/browser-check for the kept-alive dev instance that drives it.
import { app, BrowserWindow, ipcMain, shell, webContents } from "electron";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { keepSessionCookies } from "./sessionCookies.mjs";
import { applyPermissionPolicy } from "./permissions.mjs";
import { isProfilePartition, profilePartition } from "./profiles.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const dev = process.env.BRIGADIER_DEV === "1";
const unfocused = process.env.BRIGADIER_UNFOCUSED === "1";
const rendererUrl = process.env.BRIGADIER_RENDERER_URL ?? "http://localhost:1431/";

// Synchronously, before anything reads it: this name becomes the Keychain item ("<name> Safe Storage").
app.setName(dev ? "Brigadier Dev" : "Brigadier");
if (dev) {
  const data = process.env.BRIGADIER_DEV_DATA;
  if (!data) throw new Error("BRIGADIER_DEV_DATA is required in dev: dev must never use a real data folder");
  app.setPath("userData", data);
  app.setPath("crashDumps", path.join(data, "crashes"));
  app.setAppLogsPath(path.join(data, "logs"));
  // Dev is ad-hoc signed, and would raise Keychain dialogs. The real item needs a signed build.
  app.commandLine.appendSwitch("use-mock-keychain");
}
app.enableSandbox();
if (!app.requestSingleInstanceLock()) app.exit(0);

const eventsFile = process.env.BRIGADIER_EVENTS ?? null;
function note(kind, detail = {}) {
  if (!eventsFile) return;
  fs.appendFileSync(eventsFile, `${JSON.stringify({ at: new Date().toISOString(), kind, ...detail })}\n`);
}

app.on("render-process-gone", (_event, contents, details) => {
  note("render-process-gone", { webContents: contents.id, type: contents.getType(), url: contents.getURL(), ...details });
});
app.on("child-process-gone", (_event, details) => note("child-process-gone", details));
for (const name of ["browser-window-focus", "browser-window-blur", "did-become-active", "did-resign-active", "activate"]) {
  app.on(name, () => note(name));
}

app.on("session-created", (created) => {
  if (!isProfilePartition(created.storagePath)) return;
  applyPermissionPolicy(created, { note });
  keepSessionCookies(created, { note });
  if (unfocused) note("session-unfocused-policy", { path: created.storagePath });
});

app.on("web-contents-created", (_event, contents) => {
  note("web-contents-created", { webContents: contents.id, type: contents.getType() });
  contents.once("destroyed", () => note("web-contents-destroyed", { webContents: contents.id }));
  contents.on("unresponsive", () => note("unresponsive", { webContents: contents.id }));
  contents.on("responsive", () => note("responsive", { webContents: contents.id }));
  if (dev && contents.getType() !== "webview") {
    // Reloading the app's page destroys every tab in it, so each load of it is on the record.
    contents.on("did-start-navigation", (details) => {
      if (details.isMainFrame) note("host-navigation", { webContents: contents.id, url: details.url, inPlace: details.isSameDocument });
    });
    contents.on("console-message", (details) => {
      if (details.message.startsWith("[vite]")) note("vite-client", { webContents: contents.id, message: details.message.slice(0, 200) });
      if (details.level === "error") note("console-error", { webContents: contents.id, message: details.message.slice(0, 400), source: details.sourceId?.split("?")[0], line: details.lineNumber });
    });
  }
  if (contents.getType() === "webview") {
    // The Local Network prompt comes from WebRTC's local candidates; dev must show no dialog.
    if (unfocused) contents.setWebRTCIPHandlingPolicy("disable_non_proxied_udp");
    contents.on("did-start-navigation", (details) => {
      if (details.isMainFrame) note("guest-navigation", { webContents: contents.id, url: details.url, inPlace: details.isSameDocument, reload: contents.getURL() === details.url });
    });
    contents.setWindowOpenHandler((details) => {
      // A popup becomes a tab of the opener; a page that needs its opener alive comes with section 2.
      const host = contents.hostWebContents;
      host?.send("brigadier:guest-popup", { from: contents.id, url: details.url });
      return { action: "deny" };
    });
  }
});

app.on("web-contents-created", (_event, contents) => {
  contents.on("will-attach-webview", (_attach, preferences) => {
    delete preferences.preload;
    preferences.nodeIntegration = false;
    preferences.nodeIntegrationInSubFrames = false;
    preferences.contextIsolation = true;
    preferences.sandbox = true;
    preferences.webSecurity = true;
    preferences.allowRunningInsecureContent = false;
  });
});

/** The app's windows, by the name the checks know them by. */
const windows = new Map();

function createWindow({ name = "main", bounds = {}, query = "" } = {}) {
  const window = new BrowserWindow({
    width: 1280,
    height: 800,
    ...bounds,
    show: false,
    title: dev ? "Brigadier Dev" : "Brigadier",
    backgroundColor: "#0b0b0c",
    titleBarStyle: "hiddenInset",
    focusable: !unfocused,
    minimizable: true,
    webPreferences: {
      preload: path.join(here, "preload.cjs"),
      sandbox: true,
      contextIsolation: true,
      nodeIntegration: false,
      webviewTag: true,
      // One localStorage per named window: the checks run a session in each, as separate users would.
      partition: `persist:shell-${name}`,
    },
  });
  windows.set(name, window);
  for (const event of ["show", "hide", "minimize", "restore", "focus", "blur", "enter-full-screen", "leave-full-screen"]) {
    window.on(event, () => note(`window-${event}`, { window: name }));
  }
  window.on("closed", () => windows.delete(name));
  window.webContents.setWindowOpenHandler(({ url }) => {
    if (/^https?:/.test(url)) void shell.openExternal(url);
    return { action: "deny" };
  });
  if (unfocused) window.setIgnoreMouseEvents(true);
  void window.loadURL(`${rendererUrl}${query ? `?${query}` : ""}`);
  return window;
}

// The renderer's tab model: one JSON file. Which profile a tab belongs to has to survive a restart.
const tabsFile = () => path.join(app.getPath("userData"), "browser-tabs.json");
ipcMain.handle("brigadier:tabs-read", () => {
  try {
    return JSON.parse(fs.readFileSync(tabsFile(), "utf8"));
  } catch {
    return {};
  }
});
ipcMain.handle("brigadier:tabs-write", (_event, tabs) => {
  fs.writeFileSync(tabsFile(), JSON.stringify(tabs));
});
ipcMain.handle("brigadier:profile-partition", (_event, profileId) => profilePartition(profileId));
ipcMain.handle("brigadier:guest-info", (_event, id) => {
  const guest = webContents.fromId(id);
  return guest ? { id, pid: guest.getOSProcessId(), url: guest.getURL() } : null;
});

if (dev) {
  const { installDevControl } = await import("./devControl.mjs");
  installDevControl({ createWindow, windows, note });
}

// Not awaited at the top level: Electron won't reach "ready" until this module has finished loading.
void app.whenReady().then(() => {
  if (dev) {
    note("ready", { pid: process.pid, electron: process.versions.electron, chrome: process.versions.chrome, sandboxForced: true });
    // The first window waits for the driver: the checks place each window before it shows.
    const control = new BrowserWindow({
      show: false,
      webPreferences: { preload: path.join(here, "preload.cjs"), sandbox: true, contextIsolation: true },
    });
    windows.set("control", control);
    void control.loadFile(path.join(here, "control.html"));
    return;
  }
  const main = createWindow({});
  main.once("ready-to-show", () => main.show());
  app.on("activate", () => { if (windows.size === 0) createWindow({}).show(); });
});
app.on("window-all-closed", () => { if (process.platform !== "darwin") app.quit(); });
