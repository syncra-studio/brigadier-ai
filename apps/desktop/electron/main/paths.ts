// Where Chromium keeps its own files, away from the daemon's.
import path from "node:path";

/**
 * Chromium's folder, when its default can't stay. On macOS the default is `Application
 * Support/<app name>`, which is exactly the daemon's data folder (crates/sandbox/src/paths.rs:165),
 * so it goes beside it under the bundle id, a path Uninstall already covers (footprint.rs:52). Windows'
 * %APPDATA%\Brigadier and Linux's ~/.config/Brigadier are not the daemon's %LOCALAPPDATA%\Brigadier\Data
 * and ~/.local/share/brigadier, so those stay.
 */
export function chromiumFolder(platform: NodeJS.Platform, appData: string, identifier: string): string | null {
  return platform === "darwin" ? path.join(appData, identifier) : null;
}

/**
 * Windows: whether an install folder leaves the daemon's data alone. The uninstaller removes its install folder
 * recursively, and data lives in %LOCALAPPDATA%\\Brigadier\\Data (crates/sandbox/src/paths.rs:172), so the folder
 * must not be that one or any above it.
 */
export function installFolderLeavesData(installFolder: string, dataFolder: string): boolean {
  const inside = path.win32.relative(path.win32.normalize(installFolder), path.win32.normalize(dataFolder));
  return inside === "" ? false : inside.startsWith("..") || path.win32.isAbsolute(inside);
}

/** The folders opened with a launch among its arguments; an option's value is never one (#67, #97). */
export function folderArgs(args: readonly string[], cwd: string, isFolder: (path: string) => boolean): string[] {
  const folders: string[] = [];
  for (let at = 0; at < args.length; at++) {
    const arg = args[at];
    if (arg === undefined) continue;
    if (arg === "--brigadier-data-dir") {
      at++;
    } else if (!arg.startsWith("-")) {
      const full = path.resolve(cwd, arg);
      if (isFolder(full)) folders.push(full);
    }
  }
  return folders;
}
