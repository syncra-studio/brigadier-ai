#!/usr/bin/env node
// Builds Brigadier Dev.app from the official Electron release: checksum verified, renamed, its fuses
// set, its Info.plist made background-only, ad-hoc signed. Stock Electron code, nothing patched.
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

import { appBundle, binDir, buildDir, checkInputApp, desktopDir } from "./paths.mjs";

const VERSION = "45.2.3";
const NAME = "Brigadier Dev";
const ID = "ai.brigadier.dev";
const arch = process.arch === "arm64" ? "arm64" : "x64";
const zipName = `electron-v${VERSION}-darwin-${arch}.zip`;
const base = `https://github.com/electron/electron/releases/download/v${VERSION}`;

// The fuse block's layout: sentinel, version byte, length byte, one ASCII byte per fuse.
const SENTINEL = "dL7pKGdnNz796PbbjQWNKmHXBZaB9tsX";
const FUSES = [
  ["runAsNode", "0"],
  ["cookieEncryption", "1"],
  ["nodeOptions", "0"],
  ["nodeCliInspect", "0"],
  ["embeddedAsarIntegrity", null],
  ["onlyLoadAppFromAsar", null],
  ["browserV8Snapshot", null],
  ["grantFileProtocolPrivileges", "0"],
  ["wasmTrapHandlers", null],
];

const run = (file, args, options = {}) => execFileSync(file, args, { stdio: "pipe", ...options }).toString();
const plist = (file, key, type, value) => {
  try {
    run("plutil", ["-replace", key, `-${type}`, value, file]);
  } catch {
    run("plutil", ["-insert", key, `-${type}`, value, file]);
  }
};

async function download() {
  const cache = path.join(buildDir, "cache");
  fs.mkdirSync(cache, { recursive: true });
  const zip = path.join(cache, zipName);
  const sums = await (await fetch(`${base}/SHASUMS256.txt`)).text();
  const line = sums.split("\n").find((each) => each.endsWith(`*${zipName}`));
  if (!line) throw new Error(`no checksum for ${zipName}`);
  const want = line.split(" ")[0];
  const have = () => createHash("sha256").update(fs.readFileSync(zip)).digest("hex");
  if (!fs.existsSync(zip) || have() !== want) {
    const response = await fetch(`${base}/${zipName}`);
    if (!response.ok) throw new Error(`download failed: ${response.status}`);
    fs.writeFileSync(zip, Buffer.from(await response.arrayBuffer()));
  }
  if (have() !== want) throw new Error(`checksum mismatch for ${zipName}`);
  return zip;
}

function setFuses(framework) {
  const data = fs.readFileSync(framework);
  const at = data.indexOf(SENTINEL);
  if (at < 0 || data.indexOf(SENTINEL, at + 1) >= 0) throw new Error("expected one fuse block");
  const start = at + SENTINEL.length;
  const length = data[start + 1];
  if (length < FUSES.length) throw new Error(`fuse block has ${length} fuses`);
  const read = {};
  FUSES.forEach(([name, value], index) => {
    if (value !== null) data[start + 2 + index] = value.charCodeAt(0);
    read[name] = String.fromCharCode(data[start + 2 + index]);
  });
  fs.writeFileSync(framework, data);
  return read;
}

function renameHelpers(contents) {
  const frameworks = path.join(contents, "Frameworks");
  for (const [suffix, id] of [["", "helper"], [" (GPU)", "helper.gpu"], [" (Plugin)", "helper.plugin"], [" (Renderer)", "helper.renderer"]]) {
    const from = path.join(frameworks, `Electron Helper${suffix}.app`);
    const to = path.join(frameworks, `${NAME} Helper${suffix}.app`);
    fs.renameSync(from, to);
    fs.renameSync(path.join(to, `Contents/MacOS/Electron Helper${suffix}`), path.join(to, `Contents/MacOS/${NAME} Helper${suffix}`));
    const info = path.join(to, "Contents/Info.plist");
    plist(info, "CFBundleExecutable", "string", `${NAME} Helper${suffix}`);
    plist(info, "CFBundleName", "string", `${NAME} Helper${suffix}`);
    plist(info, "CFBundleDisplayName", "string", `${NAME} Helper${suffix}`);
    plist(info, "CFBundleIdentifier", "string", `${ID}.${id}`);
    plist(info, "LSBackgroundOnly", "bool", "true");
  }
}

// The one thing macOS grants Accessibility to for the real-input checks. Its grant is tied to its hash:
// made once, then left alone. Delete the app to make it again, and grant it again.
function buildCheckInput() {
  if (fs.existsSync(checkInputApp)) return;
  const macos = path.join(checkInputApp, "Contents/MacOS");
  fs.mkdirSync(macos, { recursive: true });
  const name = "Brigadier Check Input";
  run("swiftc", ["-O", "-o", path.join(macos, name), new URL("./native/realinput.swift", import.meta.url).pathname]);
  fs.writeFileSync(path.join(checkInputApp, "Contents/Info.plist"), `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>ai.brigadier.check-input</string>
<key>CFBundleName</key><string>${name}</string>
<key>CFBundleDisplayName</key><string>${name}</string>
<key>CFBundleExecutable</key><string>${name}</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleVersion</key><string>1</string>
<key>CFBundleShortVersionString</key><string>1</string>
<key>LSBackgroundOnly</key><true/>
</dict></plist>
`);
  run("codesign", ["--force", "--sign", "-", checkInputApp]);
}

if (process.argv.includes("--check-input-only")) {
  buildCheckInput();
  console.log(checkInputApp);
  process.exit(0);
}

const zip = await download();
fs.rmSync(appBundle, { recursive: true, force: true });
const unpack = path.join(buildDir, "unpack");
fs.rmSync(unpack, { recursive: true, force: true });
fs.mkdirSync(unpack, { recursive: true });
run("ditto", ["-x", "-k", zip, unpack]);
fs.renameSync(path.join(unpack, "Electron.app"), appBundle);
fs.rmSync(unpack, { recursive: true, force: true });

const contents = path.join(appBundle, "Contents");
fs.renameSync(path.join(contents, "MacOS/Electron"), path.join(contents, `MacOS/${NAME}`));
const info = path.join(contents, "Info.plist");
plist(info, "CFBundleExecutable", "string", NAME);
plist(info, "CFBundleName", "string", NAME);
plist(info, "CFBundleDisplayName", "string", NAME);
plist(info, "CFBundleIdentifier", "string", ID);
// Background only: macOS can't make it the front app, even when the front app steps aside.
plist(info, "LSBackgroundOnly", "bool", "true");
plist(info, "NSLocationWhenInUseUsageDescription", "string", "A web page you're using asks for your location.");
renameHelpers(contents);
const fuses = setFuses(path.join(contents, "Frameworks/Electron Framework.framework/Versions/A/Electron Framework"));

// The app itself stays in the repo: only this one line is inside the bundle, so a main-process edit
// needs a relaunch, not a rebuild.
const resources = path.join(contents, "Resources");
fs.rmSync(path.join(resources, "default_app.asar"), { force: true });
fs.mkdirSync(path.join(resources, "app"), { recursive: true });
fs.writeFileSync(path.join(resources, "app/package.json"), JSON.stringify({ name: "brigadier-dev", productName: NAME, version: "0.0.0-dev", main: "index.mjs", type: "module" }));
fs.writeFileSync(path.join(resources, "app/index.mjs"), `await import(${JSON.stringify(`file://${path.join(desktopDir, "dist-electron/main.mjs")}`)});\n`);

run("codesign", ["--force", "--deep", "--sign", "-", appBundle]);

// The small native tools the checks use.
fs.mkdirSync(binDir, { recursive: true });
const native = new URL("./native/", import.meta.url).pathname;
for (const file of fs.readdirSync(native)) {
  if (!file.endsWith(".swift") || file === "realinput.swift") continue;
  const out = path.join(binDir, file.replace(".swift", ""));
  if (!fs.existsSync(out) || fs.statSync(out).mtimeMs < fs.statSync(path.join(native, file)).mtimeMs) {
    run("swiftc", ["-O", "-o", out, path.join(native, file)]);
  }
}
buildCheckInput();
console.log(JSON.stringify({ app: appBundle, electron: VERSION, id: ID, fuses }, null, 1));
