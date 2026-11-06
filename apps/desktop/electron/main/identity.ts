// Who this process is, decided before anything reads it: the name becomes the Keychain item
// ("<name> Safe Storage") and, on macOS, the default for the folder that must not be used.
import path from "node:path";

import { app } from "electron";

import { chromiumFolder } from "./paths";

declare const BRIGADIER_PRODUCTION: boolean;

/** A dev run, never true in a production build, whatever the environment says. */
export const dev = !BRIGADIER_PRODUCTION && process.env.BRIGADIER_DEV === "1";
/** A dev run that must never take the focus or the pointer (L8). */
export const unfocused = dev && process.env.BRIGADIER_UNFOCUSED === "1";
export const identifier = dev ? "ai.brigadier.dev" : "ai.brigadier.app";
export const rendererUrl = dev ? (process.env.BRIGADIER_RENDERER_URL ?? "http://127.0.0.1:1431/") : "app://brigadier/index.html";

export function setIdentity(): void {
  app.setName(dev ? "Brigadier Dev" : "Brigadier");
  if (dev) {
    const data = process.env.BRIGADIER_DEV_DATA;
    if (!data) throw new Error("BRIGADIER_DEV_DATA is required in dev: dev must never use a real data folder");
    app.setPath("userData", data);
    app.setPath("crashDumps", path.join(data, "crashes"));
    app.setAppLogsPath(path.join(data, "logs"));
    // Dev is ad-hoc signed, and would raise Keychain dialogs. The real item needs a signed build.
    app.commandLine.appendSwitch("use-mock-keychain");
    return;
  }
  const folder = chromiumFolder(process.platform, app.getPath("appData"), identifier);
  if (folder) app.setPath("userData", folder);
}

/** The daemon's data folder, when this run isn't to use the installed default. */
export function daemonDataDir(): string | undefined {
  const given = process.argv.indexOf("--brigadier-data-dir");
  if (given >= 0) return process.argv[given + 1];
  if (!dev) return undefined;
  const data = process.env.BRIGADIER_DEV_DAEMON_DATA;
  if (!data) throw new Error("BRIGADIER_DEV_DAEMON_DATA is required in dev: dev must never reach the installed daemon");
  return data;
}
