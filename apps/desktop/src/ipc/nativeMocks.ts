// Stand-ins for the native shell, for tests and fixtures only. They answer `@/ipc/native` when no
// Electron preload is present, so they run in plain Node and headless Chromium under either shell.
export { clearMocks, mockIPC, mockWindows } from "@tauri-apps/api/mocks";
export { emit } from "@tauri-apps/api/event";
