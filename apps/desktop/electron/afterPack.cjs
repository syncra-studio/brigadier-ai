// macOS: Brigadier.app runs brigadier-app, as the Tauri build's does, so the CLI `brigadier` can sit beside it:
// on a case-insensitive disk `Brigadier` and `brigadier` are one file. electron-builder names the bundle after
// the executable, so the executable is renamed here, before signing, and the CLI placed after it.
const { execFileSync } = require("node:child_process");
const fs = require("node:fs");
const path = require("node:path");

exports.default = async function afterPack(context) {
  if (context.electronPlatformName !== "darwin") return;
  const contents = path.join(context.appOutDir, `${context.packager.appInfo.productFilename}.app`, "Contents");
  const macos = path.join(contents, "MacOS");
  const name = context.packager.appInfo.productFilename;
  fs.renameSync(path.join(macos, name), path.join(macos, "brigadier-app"));
  execFileSync("plutil", ["-replace", "CFBundleExecutable", "-string", "brigadier-app", path.join(contents, "Info.plist")]);
  const cli = path.join(macos, "brigadier");
  fs.copyFileSync(path.join(process.env.BRIGADIER_STAGE, "sidecars/brigadier"), cli);
  fs.chmodSync(cli, 0o755);
};
