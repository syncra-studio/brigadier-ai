export function shortcutLabel(keys: string, mac: boolean): string {
  return mac
    ? keys
    : keys
        .replace("⌃", "Ctrl+")
        .replace("⌥", "Alt+")
        .replace("⇧", "Shift+")
        .replace("⌘", "Ctrl+");
}
