#!/usr/bin/env node
// Builds a production app for one architecture: the page, main without its dev control, the Rust release for
// that target, then electron-builder.
//   node electron/dist.mjs [--arch arm64|x64] [--dir] [--check]
//     --dir: the .app only, no DMG or zip
//     --check: a package only a check runs: main keeps its dev switches (so it parks unfocused on scratch data)
//       and the app can't come to the front. Everything else is as shipped: asar, addon, sidecars in the bundle.
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const desktop = fileURLToPath(new URL("..", import.meta.url));
const root = path.resolve(desktop, "../..");
const args = process.argv.slice(2);
const flag = (name) => (args.includes(name) ? args[args.indexOf(name) + 1] : undefined);
const arch = flag("--arch") ?? (process.arch === "arm64" ? "arm64" : "x64");
const triples = { darwin: { arm64: "aarch64-apple-darwin", x64: "x86_64-apple-darwin" } };
const triple = triples[process.platform]?.[arch];
if (!triple) throw new Error(`no build configured for ${process.platform} ${arch}: Mac first; Windows and Linux need CI`);

const run = (file, list, options = {}) => execFileSync(file, list, { cwd: desktop, stdio: "inherit", ...options });
const bin = (name) => path.join(desktop, "node_modules/.bin", name);

const check = args.includes("--check");
run(process.execPath, ["electron/build.mjs", ...(check ? [] : ["--production"])]);
run(bin("vite"), ["build", "--config", "electron/vite.config.ts"]);
// The daemon, the CLI and the computer-use helper, as the Tauri build stages them.
run(process.execPath, ["scripts/stage-sidecar.mjs", "--target", triple]);
run("cargo", ["build", "--release", "--target", triple, "-p", "brigadier-shell-native", "--features", "napi"], { cwd: root });

const stage = path.join(root, "target/electron-stage", arch);
fs.rmSync(stage, { recursive: true, force: true });
fs.mkdirSync(path.join(stage, "sidecars"), { recursive: true });
fs.copyFileSync(path.join(root, "target", triple, "release/libbrigadier_shell_native.dylib"), path.join(stage, "shell_native.node"));
const binaries = path.join(desktop, "src-tauri/binaries");
for (const name of ["brigadierd", "brigadier"]) fs.copyFileSync(path.join(binaries, `${name}-${triple}`), path.join(stage, "sidecars", name));
fs.cpSync(path.join(binaries, "Brigadier Computer Use.app"), path.join(stage, "Brigadier Computer Use.app"), { recursive: true, verbatimSymlinks: true });

const dist = path.join(root, "target/electron-dist", check ? `${arch}-check` : arch);
run(bin("electron-builder"), ["--mac", ...(args.includes("--dir") || check ? ["dir"] : []), `--${arch}`, "--config", "electron/builder.config.cjs", "--publish", "never"], {
  env: { ...process.env, BRIGADIER_STAGE: stage, BRIGADIER_DIST: dist, ...(check ? { BRIGADIER_CHECK_PACKAGE: "1" } : {}), CSC_IDENTITY_AUTO_DISCOVERY: "false" },
});

if (check) {
  // Background-only, as the kept-alive dev app: macOS can't make it the front app.
  const app = path.join(dist, `mac-${arch === "x64" ? "" : "arm64"}`.replace(/-$/, ""), "Brigadier.app");
  const plists = [path.join(app, "Contents/Info.plist")];
  const frameworks = path.join(app, "Contents/Frameworks");
  for (const entry of fs.readdirSync(frameworks)) if (entry.endsWith(".app")) plists.push(path.join(frameworks, entry, "Contents/Info.plist"));
  for (const plist of plists) run("plutil", ["-replace", "LSBackgroundOnly", "-bool", "true", plist]);
  // electron-builder's `dir` target writes no update config: a check given a feed gets it here.
  if (process.env.BRIGADIER_UPDATE_FEED) fs.writeFileSync(path.join(app, "Contents/Resources/app-update.yml"), `provider: generic\nurl: ${process.env.BRIGADIER_UPDATE_FEED}\nupdaterCacheDirName: brigadier-updater\n`);
  run("codesign", ["--force", "--deep", "--sign", "-", app]);
  console.log(app);
}
