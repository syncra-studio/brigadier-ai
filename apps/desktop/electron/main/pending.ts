// Every command of the Tauri build's `invoke_handler` that the Electron shell doesn't answer yet, by the
// area that moves it. Moving one means deleting it here; commands.test.ts keeps this list honest, and Tauri
// can go only once it's empty (browser commands come by section, stage 3 on).
export const PENDING: Record<string, readonly string[]> = {
  "tray and menu": [],
  "dialogs and opening": [],
  "drops and notifications": [],
  "login and computer setup": [],
  "intro, smoke and packaging": [
  ],
};
