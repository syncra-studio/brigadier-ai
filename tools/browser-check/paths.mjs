// Where the dev app and its scratch data live. Nothing here points at /Applications or the daemon's folder.
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

export const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
export const desktopDir = path.join(repoRoot, "apps/desktop");
export const electronDir = path.join(desktopDir, "electron");
export const buildDir = path.join(repoRoot, "target/browser-check");
export const binDir = path.join(buildDir, "bin");
export const checkInputApp = path.join(binDir, "Brigadier Check Input.app");
export const appBundle = path.join(buildDir, "Brigadier Dev.app");
export const appBinary = path.join(appBundle, "Contents/MacOS/Brigadier Dev");
/** Scratch data and evidence: never the repo. */
export const checkDir = process.env.BRIGADIER_CHECK_DIR ?? path.join(process.env.TMPDIR ?? os.tmpdir(), "browser-check");
export const dataDir = path.join(checkDir, "data");
/** The dev daemon's data and, unless a check gives another, its HOME: scratch, never the user's. */
export const daemonDataDir = path.join(checkDir, "daemon-data");
export const daemonHome = path.join(checkDir, "daemon-home");
export const socketPath = path.join(checkDir, "devd.sock");
export const eventsPath = path.join(checkDir, "events.jsonl");
export const evidenceDir = path.join(checkDir, "evidence");
export const vitePort = 1431;
/** The Rust addon, as main loads it in a dev run. */
export const addonPath = path.join(buildDir, "shell_native.node");
/** Where a dev run finds `brigadierd` and `brigadier`. */
export const sidecarDir = path.join(buildDir, "sidecars");
