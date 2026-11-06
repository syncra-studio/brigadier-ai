// What login.ts decides without Electron: the same answers as src-tauri/src/login_item.rs and computer_setup.rs.

/** Whether Brigadier opens at login, as the system says. */
export type LoginItem = "unsupported" | "off" | "on" | "needsApproval";

/** SMAppService's status as Electron names it. Never registered reads as not found: that is off. */
export function macLoginItem(status: string): LoginItem {
  switch (status) {
    case "not-registered":
    case "not-found":
      return "off";
    case "enabled":
      return "on";
    case "requires-approval":
      return "needsApproval";
    default:
      return "unsupported";
  }
}

/** Whether the login item now stands as asked. Electron drops SMAppService's error, so this is how a failure shows. */
export const loginItemTook = (on: boolean, now: LoginItem): boolean => (on ? now === "on" || now === "needsApproval" : now === "off");

/** The list `SMAppService.openSystemSettingsLoginItems` opens; Electron has no call for it. */
export const LOGIN_ITEMS_PANE = "x-apple.systempreferences:com.apple.LoginItems-Settings.extension";

const PRIVACY_PANES: Record<string, string> = {
  accessibility: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
  screenRecording: "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture",
};

/** System Settings' privacy list that grants `permission`. */
export function privacyPane(permission: unknown): string {
  const pane = typeof permission === "string" && Object.hasOwn(PRIVACY_PANES, permission) ? PRIVACY_PANES[permission] : undefined;
  if (!pane) throw new Error(`unknown permission ${String(permission)}`);
  return pane;
}

/**
 * One argument of a desktop entry's `Exec`, quoted as the Desktop Entry spec says: inside double quotes
 * `"`, `` ` ``, `$` and `\` take a backslash, a literal `%` is `%%`, and then the value's own string
 * escaping doubles every backslash.
 */
export function execArg(arg: string): string {
  const quoted = `"${arg.replace(/["`$\\]/g, "\\$&").replaceAll("%", "%%")}"`;
  return quoted.replaceAll("\\", "\\\\");
}

/** An XDG autostart entry that starts `exe`. */
export const desktopEntry = (name: string, exe: string): string =>
  `[Desktop Entry]\nType=Application\nName=${name}\nExec=${execArg(exe)}\nX-GNOME-Autostart-enabled=true\n`;

/** `$XDG_CONFIG_HOME/autostart`, else `~/.config/autostart`; a relative XDG_CONFIG_HOME doesn't count. */
export function autostartDir(env: { XDG_CONFIG_HOME?: string; HOME?: string }): string | null {
  const config = env.XDG_CONFIG_HOME?.startsWith("/") ? env.XDG_CONFIG_HOME : env.HOME ? `${env.HOME}/.config` : null;
  return config && `${config}/autostart`;
}
