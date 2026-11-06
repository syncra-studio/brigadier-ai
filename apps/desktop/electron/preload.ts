// The app's own pages only; guests get no preload (see will-attach-webview in main/browserHost.ts).
import { contextBridge, ipcRenderer, webFrame, webUtils } from "electron";

import frameHtml from "../src-tauri/src/visuals_frame.html?raw";

// A subframe gets no door, only the lockdown, before any of its own script. CSP doesn't cover WebRTC, and a
// visual's srcdoc child is a fresh window the frame's own script never ran in (src-tauri/src/visuals.rs).
if (!process.isMainFrame) void webFrame.executeJavaScript(frameHtml.slice(frameHtml.indexOf("<script>") + 8, frameHtml.indexOf("</script>")));

if (process.isMainFrame) contextBridge.exposeInMainWorld("brigadierShell", {
  profiles: (op: string, args: unknown) => ipcRenderer.invoke("brigadier:profiles", op, args),
  permissionAnswer: (request: number, answer: string) => ipcRenderer.invoke("brigadier:permission-answer", request, answer),
  permissionOpenSettings: (device: string) => ipcRenderer.invoke("brigadier:permission-open-settings", device),
  flushStorage: () => ipcRenderer.send("brigadier:flush-storage"),
  profilePartition: (profileId: string) => ipcRenderer.invoke("brigadier:profile-partition", profileId),
  guestInfo: (id: number) => ipcRenderer.invoke("brigadier:guest-info", id),
  guestFavicon: (id: number, href: string) => ipcRenderer.invoke("brigadier:guest-favicon", id, href),
  onGuestPasskey: (listener: (details: unknown) => void) => ipcRenderer.on("brigadier:guest-passkey", (_event, details) => listener(details)),
  onGuestPopup: (listener: (details: unknown) => void) => ipcRenderer.on("brigadier:guest-popup", (_event, details) => listener(details)),
  onGuestPermission: (listener: (details: unknown) => void) => ipcRenderer.on("brigadier:guest-permission", (_event, details) => listener(details)),
  onGuestPermissionRefused: (listener: (details: unknown) => void) => ipcRenderer.on("brigadier:guest-permission-refused", (_event, details) => listener(details)),
  onGuestEscape: (listener: (details: unknown) => void) => ipcRenderer.on("brigadier:guest-escape", (_event, details) => listener(details)),
  onGuestKey: (listener: () => void) => ipcRenderer.on("brigadier:guest-key", () => listener()),
  agent: (call: unknown) => ipcRenderer.invoke("brigadier:agent", call),
  onAgentEvent: (listener: (details: unknown) => void) => ipcRenderer.on("brigadier:agent-event", (_event, details) => listener(details)),
  devControl: (op: string, args: unknown) => ipcRenderer.invoke("brigadier:dev-control", op, args),
});

// A stand-in window answers its shell in the page; a real one has this door (src/ipc/native.ts).
if (process.isMainFrame && !process.argv.includes("--brigadier-stand-in")) {
  const listeners = new Map<string, Set<(payload: unknown) => void>>();
  ipcRenderer.on("brigadier:event", (_event, name: string, payload: unknown) => {
    for (const listener of listeners.get(name) ?? []) listener(payload);
  });
  contextBridge.exposeInMainWorld("brigadierNative", {
    invoke: (command: string, args?: unknown) => ipcRenderer.invoke("brigadier:invoke", command, args),
    listen: (event: string, handler: (payload: unknown) => void) => {
      const set = listeners.get(event) ?? new Set();
      listeners.set(event, set);
      set.add(handler);
      return () => set.delete(handler);
    },
    onChannel: (handler: (id: number, json: string) => void) => {
      ipcRenderer.on("brigadier:channel", (_event, id: number, json: string) => handler(id, json));
    },
    onResized: (handler: () => void) => {
      const listener = () => handler();
      ipcRenderer.on("brigadier:resized", listener);
      return () => ipcRenderer.off("brigadier:resized", listener);
    },
    pathForFile: (file: File) => webUtils.getPathForFile(file),
  });
  // What the OS really dropped, for main (main/drops.ts): the page can't reach this world or forge the event.
  window.addEventListener("drop", (event) => {
    if (!event.isTrusted) return;
    const paths = [...(event.dataTransfer?.files ?? [])].map((file) => webUtils.getPathForFile(file)).filter((path) => path !== "");
    if (paths.length > 0) ipcRenderer.send("brigadier:dropped", paths, event.shiftKey);
  }, true);
}
