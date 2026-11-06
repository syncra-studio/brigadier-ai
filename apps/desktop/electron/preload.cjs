// The app's own pages only; guests get no preload (see will-attach-webview in main.mjs).
const { contextBridge, ipcRenderer } = require("electron");

contextBridge.exposeInMainWorld("brigadierShell", {
  tabsRead: () => ipcRenderer.invoke("brigadier:tabs-read"),
  tabsWrite: (tabs) => ipcRenderer.invoke("brigadier:tabs-write", tabs),
  profilePartition: (profileId) => ipcRenderer.invoke("brigadier:profile-partition", profileId),
  guestInfo: (id) => ipcRenderer.invoke("brigadier:guest-info", id),
  onGuestPopup: (listener) => ipcRenderer.on("brigadier:guest-popup", (_event, details) => listener(details)),
  devControl: (op, args) => ipcRenderer.invoke("brigadier:dev-control", op, args),
});
