#!/usr/bin/env node
// Builds what the Electron app needs from the Rust side: the addon, copied to where main loads it from,
// and the daemon and CLI it launches. Debug unless given --release. Doesn't touch a running instance.
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

import { addonPath, buildDir, repoRoot } from "./paths.mjs";

const release = process.argv.includes("--release");
const profile = release ? ["--release"] : [];
const run = (args) => execFileSync("cargo", args, { cwd: repoRoot, stdio: "inherit" });
run(["build", ...profile, "-p", "brigadier-shell-native", "--features", "napi"]);
run(["build", ...profile, "-p", "brigadier-daemon", "-p", "brigadier-cli"]);
const library = process.platform === "darwin" ? "libbrigadier_shell_native.dylib" : process.platform === "win32" ? "brigadier_shell_native.dll" : "libbrigadier_shell_native.so";
fs.mkdirSync(buildDir, { recursive: true });
fs.copyFileSync(path.join(repoRoot, "target", release ? "release" : "debug", library), addonPath);
console.log(addonPath);
