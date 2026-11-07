#!/usr/bin/env node
// What a build came to: every download's real size, the installed app's size on disk, the parts that make it up, and
// the packaging facts this OS can show. MB = 10^6 bytes.
//   package-report.mjs <electron-dist folder for one arch>
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

const dist = process.argv[2];
const mb = (bytes) => (bytes / 1e6).toFixed(1);
const size = (file) => {
  const stat = fs.lstatSync(file);
  if (stat.isSymbolicLink()) return 0;
  return stat.isDirectory() ? fs.readdirSync(file).reduce((sum, name) => sum + size(path.join(file, name)), 0) : stat.size;
};
const rows = [];
for (const name of fs.readdirSync(dist)) if (/\.(dmg|zip|exe|deb|AppImage)$/.test(name)) rows.push([`Download: ${name}`, mb(size(path.join(dist, name)))]);
const folder = fs.readdirSync(dist).find((name) => /^(mac|win|linux)(-arm64)?(-unpacked)?$/.test(name));
const root = path.join(dist, folder);
const app = process.platform === "darwin" ? path.join(root, "Brigadier.app") : root;
rows.push(["Installed app, whole", mb(size(app))]);
const parts = process.platform === "darwin"
  ? { "Electron Framework": "Contents/Frameworks/Electron Framework.framework", Daemon: "Contents/MacOS/brigadierd", CLI: "Contents/MacOS/brigadier", "Computer-use helper binary": "Contents/Helpers/Brigadier Computer Use.app/Contents/MacOS", Addon: "Contents/Resources/shell_native.node", "App page and main": "Contents/Resources/app.asar" }
  : process.platform === "win32"
    ? { Daemon: "brigadierd.exe", CLI: "brigadier.exe", "Computer-use helper": "brigadier-cu.exe", Addon: "resources/shell_native.node", "App page and main": "resources/app.asar", Engine: "brigadier-app.exe", Locales: "locales" }
    : { Daemon: "brigadierd", CLI: "brigadier", "Computer-use helper": "brigadier-cu", Addon: "resources/shell_native.node", "App page and main": "resources/app.asar", Engine: "brigadier-app", Locales: "locales" };
for (const [name, part] of Object.entries(parts)) if (fs.existsSync(path.join(app, part))) rows.push([`  ${name}`, mb(size(path.join(app, part)))]);

if (process.platform === "darwin") {
  const resources = path.join(app, "Contents/Frameworks/Electron Framework.framework/Versions/A/Resources");
  rows.push(["  Locales", mb(fs.readdirSync(resources).filter((name) => name.endsWith(".lproj")).reduce((sum, name) => sum + size(path.join(resources, name)), 0))]);
}

const facts = [];
if (process.platform === "darwin") {
  const plist = path.join(app, "Contents/Info.plist");
  facts.push(`CFBundleDocumentTypes: ${execFileSync("plutil", ["-extract", "CFBundleDocumentTypes", "json", "-o", "-", plist], { encoding: "utf8" }).trim()}`);
  facts.push(`Architecture: ${execFileSync("lipo", ["-archs", path.join(app, "Contents/MacOS/brigadier-app")], { encoding: "utf8" }).trim()}`);
  // Reads the image's header; nothing is mounted.
  for (const name of fs.readdirSync(dist).filter((each) => each.endsWith(".dmg"))) {
    const format = execFileSync("hdiutil", ["imageinfo", "-format", path.join(dist, name)], { encoding: "utf8" }).trim();
    facts.push(`${name} format: ${format}`);
  }
}
const lines = ["## Size", "", "| What | MB |", "|---|---:|", ...rows.map(([what, value]) => `| ${what} | ${value} |`), "", ...facts.map((fact) => `- ${fact}`), ""];
console.log(lines.join("\n"));
if (process.env.GITHUB_STEP_SUMMARY) fs.appendFileSync(process.env.GITHUB_STEP_SUMMARY, `${lines.join("\n")}\n`);
