const { contextBridge, ipcRenderer } = require('electron');
contextBridge.exposeInMainWorld('pad', {
  log: (id, ev, v) => ipcRenderer.send('log', id, ev, v),
  onSnapshot: (f) => ipcRenderer.on('snapshot', () => ipcRenderer.send('state', f())),
});
