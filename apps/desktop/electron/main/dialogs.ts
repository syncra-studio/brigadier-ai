// System dialogs, handing things to the system's apps, folders opened with the app, and the visual frame's
// scheme. The commands are the Tauri build's (src-tauri/src/main.rs); the file logic is in dialogFiles.ts and
// the opening policy in the addon (crates/shell-native/src/opening.rs).
import { spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

import { app, dialog, protocol, shell, type BrowserWindow, type OpenDialogOptions, type SaveDialogOptions } from "electron";

import frameHtml from "../../src-tauri/src/visuals_frame.html?raw";

import { addCommands, type Args } from "./commands";
import { folderOpened } from "./core";
import { checkPdf, checkUrl, Documents, failure, fileManager, openWithCommand, saveCopy } from "./dialogFiles";
import { note } from "./events";
import { dev } from "./identity";
import { native } from "./native";
import { mainWindow, showMain } from "./windows";

/** What leaves the app: a panel on the screen, or another app. A dev check swaps these (see `scripted`). */
type Effects = {
  open(options: OpenDialogOptions): Promise<string[]>;
  save(options: SaveDialogOptions): Promise<string | null>;
  openPath(target: string): Promise<string>;
  openWith(target: string, application: string): Promise<void>;
  reveal(target: string): void;
  openUrl(url: string): Promise<void>;
};

const parented = <T>(ask: (window: BrowserWindow) => Promise<T>, alone: () => Promise<T>) => {
  const window = mainWindow();
  return window ? ask(window) : alone();
};

const real: Effects = {
  open: async (options) => {
    const { canceled, filePaths } = await parented((window) => dialog.showOpenDialog(window, options), () => dialog.showOpenDialog(options));
    return canceled ? [] : filePaths;
  },
  save: async (options) => {
    const { canceled, filePath } = await parented((window) => dialog.showSaveDialog(window, options), () => dialog.showSaveDialog(options));
    return canceled || !filePath ? null : filePath;
  },
  openPath: (target) => shell.openPath(target),
  // Detached, as the Tauri build's: whether the app then starts isn't this call's to report.
  openWith: (target, application) => new Promise((resolve, reject) => {
    const { command, args } = openWithCommand(process.platform, application, target);
    const child = spawn(command, args, { detached: true, stdio: "ignore" });
    child.once("error", reject);
    child.once("spawn", () => {
      child.unref();
      resolve();
    });
  }),
  reveal: (target) => shell.showItemInFolder(target),
  openUrl: (url) => shell.openExternal(url),
};

/**
 * Dev only, for a check that may show no panel and start no app (L8): answers from a JSON list in the file
 * BRIGADIER_DEV_EFFECTS names, and writes what would have happened to the events file. This proves the
 * commands' paths and the daemon's part, not that a real panel or app appears.
 */
function scripted(file: string): Effects {
  const answer = () => {
    const answers = JSON.parse(fs.readFileSync(file, "utf8")) as (string[] | string | null)[];
    const next = answers.shift() ?? null;
    fs.writeFileSync(file, JSON.stringify(answers));
    return next;
  };
  return {
    open: async (options) => {
      note("effect-open", { options });
      return [answer() ?? []].flat();
    },
    save: async (options) => {
      note("effect-save", { options });
      return answer() as string | null;
    },
    openPath: async (target) => {
      note("effect-open-path", { target });
      return "";
    },
    openWith: async (target, application) => note("effect-open-with", { target, application }),
    reveal: (target) => note("effect-reveal", { target }),
    openUrl: async (url) => note("effect-open-url", { url }),
  };
}

const effects = dev && process.env.BRIGADIER_DEV_EFFECTS ? scripted(process.env.BRIGADIER_DEV_EFFECTS) : real;
/** A page's mailto: or tel: link, once guestLinks.ts has seen the user's press. Thread links go through checkUrl. */
export const openWithOs = (url: string): Promise<void> => effects.openUrl(url);

const isFolder = (candidate: string | null | undefined): candidate is string => {
  if (!candidate) return false;
  try {
    return fs.statSync(candidate).isDirectory();
  } catch {
    return false;
  }
};

/** Save panels offer the name in the folder given. macOS shows `message`, and `title` elsewhere. */
function saveOptions(title: string, directory: string | null | undefined, name: string): SaveDialogOptions {
  return {
    title,
    message: title,
    defaultPath: directory ? path.join(directory, name) : name,
    properties: ["createDirectory", "showOverwriteConfirmation"],
  };
}

async function daemon(request: Record<string, unknown>): Promise<Record<string, unknown>> {
  const outcome = await native().request(JSON.stringify(request));
  if (outcome.error !== undefined && outcome.error !== null) throw JSON.parse(outcome.error);
  return JSON.parse(outcome.response ?? "null") as Record<string, unknown>;
}

/** Opens `target` as the addon's policy allows (#770): one that would run something is shown instead. */
async function openChecked(target: string, application: string | null | undefined): Promise<void> {
  const decision = native().decideOpening(target, Boolean(application));
  if (decision.kind === "refuse") throw failure("invalid", decision.detail ?? "");
  if (decision.kind === "reveal") {
    const name = decision.detail ?? "";
    try {
      await revealed(target);
    } catch (error) {
      throw failure("internal", `${name} wasn't opened, as it could run a program, nor shown: ${String(error)}`);
    }
    throw failure("invalid", `${name} wasn't opened: it could run a program. It's shown in ${fileManager(process.platform)}.`);
  }
  try {
    if (application) await effects.openWith(target, application);
    else {
      const failed = await effects.openPath(target);
      if (failed) throw new Error(failed);
    }
  } catch (error) {
    throw failure("internal", `could not open it: ${error instanceof Error ? error.message : String(error)}`);
  }
}

/** Shows a path's real target selected in the file manager, or fails when nothing is there. */
async function revealed(target: string): Promise<void> {
  effects.reveal(await fs.promises.realpath(target));
}

const documents = new Documents(async ({ directory, name }) => effects.save(saveOptions("Save file", directory, name)));

/** The scheme's standing, which main must declare in its one `registerSchemesAsPrivileged` before ready. */
export const visualScheme = { scheme: "brigadier-ui", privileges: { standard: true, secure: true } };

const FRAME_POLICY = "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data: blob:; font-src data:; connect-src 'none'; form-action 'none'; base-uri 'none'; frame-src 'none'; child-src 'none'; worker-src 'none'; object-src 'none'";

/** The visual frame's one document. Only the app's own session has it: browser tabs have their own. */
function serveVisualFrame(): void {
  protocol.handle(visualScheme.scheme, (request) => {
    const found = new URL(request.url).pathname === "/frame";
    return new Response(found ? frameHtml : "This visual cannot run in this webview.", {
      status: found ? 200 : 404,
      headers: { "Content-Type": "text/html; charset=utf-8", "X-DNS-Prefetch-Control": "off", "Content-Security-Policy": FRAME_POLICY },
    });
  });
}

let bridgeUp = false;

/** The bridge is up: a folder opened from now on is told to the page, which took the earlier ones when it connected. */
export function dialogsBridgeUp(): void {
  bridgeUp = true;
}

/** Folders opened with the app. Anything but an existing folder is ignored. */
export function openFolders(paths: string[]): void {
  const folders = paths.filter(isFolder);
  if (folders.length === 0) return;
  folderOpened(folders);
  note("folders-opened", { folders, told: bridgeUp });
  if (!bridgeUp) return;
  showMain();
  native().emit(JSON.stringify({ type: "foldersOpened" }));
}

export function installDialogs(): void {
  // Finder, the Dock and `open -a`. A launch by opening a folder delivers it before the app is ready.
  app.on("open-file", (event, opened) => {
    event.preventDefault();
    note("open-file", { path: opened });
    openFolders([opened]);
  });
  void app.whenReady().then(serveVisualFrame);
  addCommands({
    pick_folder: async ({ starting }: Args) => {
      const [folder] = await effects.open({
        title: "Choose a folder",
        message: "Choose a folder",
        ...(isFolder(starting as string | null) ? { defaultPath: starting as string } : {}),
        properties: ["openDirectory", "createDirectory"],
      });
      return folder ?? null;
    },
    // Nothing to do yet: Electron has no call that makes a panel without showing it. The Tauri build makes one
    // here so the first pick opens about half a second sooner; this needs a line in the addon.
    warm_folder_picker: () => null,
    pick_file: async ({ title, starting }: Args) => {
      const [file] = await effects.open({
        title: title as string,
        message: title as string,
        ...(isFolder(starting as string | null) ? { defaultPath: starting as string } : {}),
        properties: ["openFile"],
      });
      return file ?? null;
    },
    pick_folders: () => effects.open({ title: "Open Folder", message: "Open Folder", properties: ["openDirectory", "createDirectory", "multiSelections"] }),
    save_artifact: async ({ id, fileName }: Args) => {
      const destination = await effects.save(saveOptions("Save to…", null, fileName as string));
      if (destination === null) return false;
      await daemon({ method: "saveArtifact", id, path: destination });
      return true;
    },
    export_conventions: async ({ projectId, directory }: Args) => {
      const starting = isFolder(directory as string | null) ? (directory as string) : null;
      const destination = await effects.save(saveOptions("Export conventions", starting, "AGENTS.md"));
      if (destination === null) return null;
      return (await daemon({ method: "exportConventions", projectId, path: destination })).export;
    },
    open_artifact: async ({ id, fileName }: Args) => {
      const { path: copy } = await daemon({ method: "openArtifact", id, fileName });
      await openChecked(copy as string, null);
    },
    open_folder: ({ path: target, with: application }: Args) => openChecked(target as string, application as string | null),
    open_url: async ({ url }: Args) => {
      checkUrl(url as string);
      try {
        await effects.openUrl(url as string);
      } catch (error) {
        throw failure("internal", `could not open it: ${error instanceof Error ? error.message : String(error)}`);
      }
    },
    open_pdf: async ({ path: target }: Args) => {
      await checkPdf(target as string);
      const failed = await effects.openPath(target as string);
      if (failed) throw failure("internal", `could not open it: ${failed}`);
    },
    reveal_path: async ({ path: target }: Args) => {
      try {
        await revealed(target as string);
      } catch (error) {
        throw failure("internal", `could not show it: ${error instanceof Error ? error.message : String(error)}`);
      }
    },
    save_document: ({ id, text, directory, name, choose }: Args) => documents.save({
      id: id as string,
      text: text as string,
      directory: (directory as string | null) ?? null,
      name: name as string,
      choose: Boolean(choose),
    }),
    forget_document: ({ id }: Args) => documents.forget(id as string),
    save_copy: ({ name, bytes, path: source, ask }: Args) => saveCopy({
      name: name as string,
      bytes: bytes as Uint8Array | number[] | null,
      source: source as string | null,
      downloads: () => app.getPath("downloads"),
      choose: ask ? ({ directory, name: suggested }) => effects.save(saveOptions("Download image", directory, suggested)) : null,
    }),
  });
}
