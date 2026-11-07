#!/usr/bin/env node
// Builds what the Electron app needs from the Rust side: the addon, copied to where main loads it from,
// and the daemon and CLI it launches. Debug unless given --release. Doesn't touch a running instance.
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

import { addonPath, buildDir, repoRoot, sidecarDir } from "./paths.mjs";

const release = process.argv.includes("--release");
const profile = release ? ["--release"] : [];
const run = (args) => execFileSync("cargo", args, { cwd: repoRoot, stdio: "inherit" });
run(["build", ...profile, "-p", "brigadier-shell-native", "--features", "napi"]);
// Tauri's build script overwrites target/debug/brigadierd with its empty sidecar placeholder, so a dev run
// keeps its own copies, taken from what cargo has just built.
const out = path.join(repoRoot, "target", release ? "release" : "debug");
const names = ["brigadierd", "brigadier"];
for (const name of names) fs.rmSync(path.join(out, name), { force: true });
run(["build", ...profile, "-p", "brigadier-daemon", "-p", "brigadier-cli"]);
fs.mkdirSync(sidecarDir, { recursive: true });
for (const name of names) {
  fs.rmSync(path.join(sidecarDir, name), { force: true });
  fs.copyFileSync(path.join(out, name), path.join(sidecarDir, name));
}
const library = process.platform === "darwin" ? "libbrigadier_shell_native.dylib" : process.platform === "win32" ? "brigadier_shell_native.dll" : "libbrigadier_shell_native.so";
fs.mkdirSync(buildDir, { recursive: true });
fs.copyFileSync(path.join(out, library), addonPath);
console.log(addonPath);
