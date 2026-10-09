// The computer-use suite's Electron fixture (docs/COMPUTER-USE-PLAN.md §8, Phase 5, stream B).
//
// One page with a field, a checkbox and buttons. Every control writes what happened to it to a
// log, one JSON object a line, as the other fixtures do.
//
//   Electron <this dir> <log path> <scratch user data dir>
//
// It never activates itself and never shows over the user's work: it is opened in the background
// (`open -n -g`), and its window is ordered behind every other window by the order_back addon
// (Electron's showInactive() would order it to the front). SIGUSR1 logs the page's state;
// SIGTERM quits.

const { app, BrowserWindow, ipcMain } = require('electron');
const fs = require('fs');
const path = require('path');

const logPath = process.argv[2] || '/tmp/electron-pad.log';
const dataDir = process.argv[3] || '/tmp/electron-pad-data';
app.setPath('userData', dataDir);
fs.writeFileSync(logPath, '');
if (process.env.FIXTURE_PID_FILE) fs.writeFileSync(process.env.FIXTURE_PID_FILE, String(process.pid));

function log(id, ev, v) {
  const o = { ev, id, t: Date.now() };
  if (v !== undefined) o.v = String(v);
  fs.appendFileSync(logPath, JSON.stringify(o) + '\n');
}

const native = require(process.env.ELECTRON_PAD_ADDON || path.join(__dirname, 'order_back.node'));
let win;

ipcMain.on('log', (_e, id, ev, v) => log(id, ev, v));
ipcMain.on('state', (_e, state) => log('state', 'snapshot', JSON.stringify(state)));

app.whenReady().then(() => {
  win = new BrowserWindow({
    show: false,
    width: 460,
    height: 300,
    title: 'Electron Pad',
    webPreferences: { preload: path.join(__dirname, 'preload.js') },
  });
  win.loadFile(path.join(__dirname, 'index.html'));
  win.once('ready-to-show', () => {
    log('window', native.orderBack(win.getNativeWindowHandle()) ? 'ordered-back' : 'not-ordered');
  });
  app.on('browser-window-focus', () => log('app', 'focus'));
  log('app', 'ready', process.pid);
});

process.on('SIGUSR1', () => win && win.webContents.send('snapshot'));
process.on('SIGTERM', () => { log('app', 'quit'); app.exit(0); });
app.on('window-all-closed', () => app.quit());
