// The browser's security foundations and the record of its guests, from stage 1. Browser features come by
// section (L4); per-frame loops go in stage 3 (L35).
import fs from "node:fs";
import path from "node:path";

import { app, webContents } from "electron";

import { handleFromApp, isAppPage } from "./appPage";
import { dev, unfocused } from "./identity";
import { note } from "./events";
import { installGuestFavicon } from "./guestFavicon";
import { installGuest, webOrBlank } from "./guestPopups";
import { installGuestKeys } from "./guestKeys";
import { applyAppSessionPolicy, applyPermissionPolicy } from "./permissions";
import { isProfilePartition, profilePartition } from "./profiles";
import { installDurableStorage, keepStorageOf, replaceFile } from "./durableStorage";
import { keepSessionCookies } from "./sessionCookies";
import { guestPasskeyPath } from "./windows";

// The renderer's tab model: one JSON file. Which profile a tab belongs to has to survive a restart.
const tabsFile = () => path.join(app.getPath("userData"), "browser-tabs.json");
/** The partitions main has named for a profile: the only sessions a guest may have. */
const profileSessions = new Set<string>();

export function installBrowserHost(): void {
  installDurableStorage();
  installGuestFavicon();
  app.on("render-process-gone", (_event, contents, details) => {
    note("render-process-gone", { webContents: contents.id, type: contents.getType(), url: contents.getURL(), ...details });
  });
  app.on("child-process-gone", (_event, details) => note("child-process-gone", { ...details }));
  for (const name of ["browser-window-focus", "browser-window-blur", "did-become-active", "did-resign-active", "activate"] as const) {
    app.on(name as "activate", () => note(name));
  }

  app.on("session-created", (created) => {
    if (!isProfilePartition(created.storagePath)) {
      applyAppSessionPolicy(created, { note, isAppPage });
      return;
    }
    applyPermissionPolicy(created, { note });
    keepSessionCookies(created, { note });
    if (process.platform === "darwin") created.registerPreloadScript({ type: "frame", filePath: guestPasskeyPath });
    if (unfocused) note("session-unfocused-policy", { path: created.storagePath });
  });

  app.on("web-contents-created", (_event, contents) => {
    note("web-contents-created", { webContents: contents.id, type: contents.getType() });
    keepStorageOf(contents);
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
      installGuestKeys(contents);
      contents.ipc.on("brigadier:guest-passkey-refused", () => {
        note("guest-passkey-refused", { webContents: contents.id });
        contents.hostWebContents?.send("brigadier:guest-passkey", { from: contents.id });
      });
      contents.on("did-start-navigation", (details) => {
        if (details.isMainFrame) note("guest-navigation", { webContents: contents.id, url: details.url, inPlace: details.isSameDocument, reload: contents.getURL() === details.url });
      });
      installGuest(contents);
    }
    contents.on("will-attach-webview", (attach, preferences, params) => {
      if (!profileSessions.has(params.partition ?? "") || !webOrBlank(params.src ?? "")) {
        note("guest-refused", { host: contents.id, partition: params.partition, src: params.src });
        attach.preventDefault();
        return;
      }
      delete preferences.preload;
      preferences.nodeIntegration = false;
      preferences.nodeIntegrationInSubFrames = false;
      preferences.contextIsolation = true;
      preferences.sandbox = true;
      preferences.webSecurity = true;
      preferences.allowRunningInsecureContent = false;
    });
  });

  handleFromApp("brigadier:tabs-read", () => {
    try {
      return JSON.parse(fs.readFileSync(tabsFile(), "utf8"));
    } catch {
      return {};
    }
  });
  handleFromApp("brigadier:tabs-write", (_event, tabs) => {
    replaceFile(tabsFile(), JSON.stringify(tabs));
  });
  handleFromApp("brigadier:profile-partition", (_event, profileId: string) => {
    const partition = profilePartition(profileId);
    profileSessions.add(partition);
    return partition;
  });
  handleFromApp("brigadier:guest-info", (_event, id: number) => {
    const guest = webContents.fromId(id);
    return guest ? { id, pid: guest.getOSProcessId(), url: guest.getURL() } : null;
  });
}
