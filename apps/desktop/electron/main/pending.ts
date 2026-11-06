// Every command of the Tauri build's `invoke_handler` that the Electron shell doesn't answer yet, by the
// area that moves it. Moving one means deleting it here; commands.test.ts keeps this list honest, and Tauri
// can go only once it's empty (browser commands come by section, stage 3 on).
export const PENDING: Record<string, readonly string[]> = {
  "tray and menu": [],
  "dialogs and opening": [
    "pick_folder", "warm_folder_picker", "pick_folders", "pick_file", "save_artifact", "export_conventions",
    "open_artifact", "open_folder", "open_url", "open_pdf", "reveal_path", "save_document", "save_copy", "forget_document",
  ],
  "drops and notifications": [
    "drag_shift", "last_drop", "read_dropped", "notification_permission", "ask_notification_permission", "open_notification_settings",
  ],
  "login and computer setup": [
    "login_item", "set_login_item", "open_login_items", "open_privacy_settings", "open_computer_setup",
    "computer_setup_subscribe", "computer_setup_drag", "reveal_computer_helper",
  ],
  "intro, smoke and packaging": [
    "intro_begin", "intro_shaped", "intro_finish", "intro_release", "smoke_finish",
    "app_update", "check_for_updates", "take_app_update", "restart_to_update",
  ],
};
