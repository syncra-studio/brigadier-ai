// Builds brigadierd and the `brigadier` command and stages them where Tauri's `externalBin`
// expects them: src-tauri/binaries/<name>-<target-triple>[.exe]. Both land next to the app's own
// program (brigadier-app), and that folder is first on every session's PATH.
//
// Runs automatically from beforeDevCommand / beforeBuildCommand. The target comes from
// TAURI_ENV_TARGET_TRIPLE (set by the Tauri CLI), then `--target <triple>`, then the host.
// For `universal-apple-darwin` both architectures are built and merged with `lipo`; the
// per-architecture binaries are staged too because each architecture's build script
// copies its own sidecar.
//
// It also builds the computer-use helper from helpers/computer-use. On macOS it assembles
// src-tauri/binaries/Brigadier Computer Use.app, which bundle.macOS.files puts in
// Contents/Helpers. Its bundle id is `--helper-id` (the dev config passes the dev id) and never
// changes, because macOS keys the Accessibility and Screen Recording grants to it. It is signed
// with APPLE_SIGNING_IDENTITY when set, otherwise ad hoc. On Windows and Linux the helper is one
// more sidecar, `brigadier-cu`, staged like the others (tauri.windows/linux.conf.json list it).

import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { copyFileSync, chmodSync, cpSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { canSandbox, nestedSwiftEnv } from "./nested-swift.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "../../..");
const binariesDir = resolve(here, "../src-tauri/binaries");
// Where cargo puts its output: CARGO_TARGET_DIR (relative to the repo, where cargo runs) or target/.
const targetDir = resolve(repoRoot, process.env.CARGO_TARGET_DIR || "target");
// The daemon, and the command sessions call.
const names = ["brigadierd", "brigadier"];

const args = process.argv.slice(2);
const flag = (key) => {
  const index = args.indexOf(key);
  return index >= 0 ? args[index + 1] : undefined;
};

function run(command, commandArgs) {
  execFileSync(command, commandArgs, { cwd: repoRoot, stdio: "inherit" });
}

function hostTriple() {
  const output = execFileSync("rustc", ["-vV"], { encoding: "utf8" });
  const line = output.split("\n").find((entry) => entry.startsWith("host: "));
  if (!line) throw new Error("could not read the host target from `rustc -vV`");
  return line.slice("host: ".length).trim();
}

const host = hostTriple();
const target = process.env.TAURI_ENV_TARGET_TRIPLE || flag("--target") || host;
const release = process.env.TAURI_ENV_DEBUG !== "true" && !args.includes("--debug");
const profile = release ? "release" : "debug";

/** Builds the daemon and the command for `triple` and returns the paths of the produced binaries. */
function build(triple) {
  // The app's package is selected too (only brigadierd and brigadier are built), so the shared
  // crates get the features they get in the app's build. A `tauri build --bundles app -- -p
  // brigadier-desktop -p brigadier-daemon` then reuses them instead of compiling the workspace a
  // second time.
  const cargoArgs = ["build", "--locked", "-p", "brigadier-daemon", "-p", "brigadier-desktop", "-p", "brigadier-cli"];
  for (const name of names) cargoArgs.push("--bin", name);
  if (release) cargoArgs.push("--release");
  // Building for the host without --target shares its build cache with other cargo commands.
  const crossTarget = triple !== host;
  if (crossTarget) cargoArgs.push("--target", triple);
  run("cargo", cargoArgs);
  const exe = triple.includes("windows") ? ".exe" : "";
  return names.map((name) => join(targetDir, crossTarget ? triple : "", profile, `${name}${exe}`));
}

function stage(source, name, triple) {
  const exe = triple.includes("windows") ? ".exe" : "";
  const destination = join(binariesDir, `${name}-${triple}${exe}`);
  copyFileSync(source, destination);
  chmodSync(destination, 0o755);
  console.log(`staged ${destination}`);
  return destination;
}

mkdirSync(binariesDir, { recursive: true });

if (target === "universal-apple-darwin") {
  const arm = build("aarch64-apple-darwin");
  const intel = build("x86_64-apple-darwin");
  names.forEach((name, at) => {
    const armBinary = stage(arm[at], name, "aarch64-apple-darwin");
    const intelBinary = stage(intel[at], name, "x86_64-apple-darwin");
    const universal = join(binariesDir, `${name}-universal-apple-darwin`);
    execFileSync("lipo", ["-create", "-output", universal, armBinary, intelBinary], { stdio: "inherit" });
    console.log(`staged ${universal}`);
  });
} else {
  build(target).forEach((binary, at) => stage(binary, names[at], target));
}

const helperName = "Brigadier Computer Use";
const helperBin = "brigadier-cu";
const helperDir = resolve(repoRoot, "helpers/computer-use");
// The helper is its own workspace; its output lives beside the main build's.
const helperTargetDir = join(targetDir, "computer-use");

/** The environment the helper builds in: inside a sandbox, SwiftPM keeps it (nested-swift.mjs). */
function helperEnv() {
  // Only the macOS helper builds Swift.
  if (!target.includes("apple-darwin") || canSandbox()) return process.env;
  console.log("SwiftPM uses the existing sandbox (--disable-sandbox)");
  return nestedSwiftEnv(repoRoot, process.env);
}

/** Builds the helper for `triple` and returns the path of the produced binary. */
function buildHelper(triple) {
  const cargoArgs = ["build", "--locked", "-p", helperBin, "--bin", helperBin];
  if (release) cargoArgs.push("--release");
  cargoArgs.push("--target-dir", helperTargetDir);
  const crossTarget = triple !== host;
  if (crossTarget) cargoArgs.push("--target", triple);
  execFileSync("cargo", cargoArgs, { cwd: helperDir, stdio: "inherit", env: helperEnv() });
  const exe = triple.includes("windows") ? ".exe" : "";
  return join(helperTargetDir, crossTarget ? triple : "", profile, `${helperBin}${exe}`);
}

/** Assembles and signs the helper bundle around `binary`. */
function stageHelper(binary) {
  const id = flag("--helper-id") || "ai.brigadier.computer-use";
  const version = JSON.parse(readFileSync(resolve(here, "../package.json"), "utf8")).version;
  const template = join(helperDir, "scripts/BrigadierCuBundle");
  const app = join(binariesDir, `${helperName}.app`);
  rmSync(app, { recursive: true, force: true });
  cpSync(template, app, { recursive: true });
  mkdirSync(join(app, "Contents/MacOS"), { recursive: true });
  copyFileSync(binary, join(app, "Contents/MacOS", helperBin));
  chmodSync(join(app, "Contents/MacOS", helperBin), 0o755);
  copyFileSync(
    resolve(here, "../src-tauri/icons/icon.icns"),
    join(app, "Contents/Resources/AppIcon.icns"),
  );
  // The build id: a running helper from any other build is replaced by this one.
  const digest = createHash("sha256").update(readFileSync(binary)).digest("hex").slice(0, 16);
  writeFileSync(join(app, "Contents/Resources/build-id"), `${version}-${digest}\n`);
  const plist = join(app, "Contents/Info.plist");
  for (const [key, value] of [
    ["CFBundleIdentifier", id],
    ["CFBundleShortVersionString", version],
    ["CFBundleVersion", version],
  ]) {
    execFileSync("plutil", ["-replace", key, "-string", value, plist], { stdio: "inherit" });
  }
  const identity = process.env.APPLE_SIGNING_IDENTITY || "-";
  const signArgs = ["--force", "--sign", identity, "--identifier", id, "--options", "runtime"];
  signArgs.push("--entitlements", join(helperDir, "scripts/BrigadierCu.entitlements"));
  // A Developer ID signature carries a secure timestamp; an ad hoc one can't.
  signArgs.push(identity === "-" ? "--timestamp=none" : "--timestamp");
  execFileSync("codesign", [...signArgs, app], { stdio: "inherit" });
  console.log(`staged ${app} (${id})`);
}

if (target.includes("apple-darwin")) {
  if (target === "universal-apple-darwin") {
    const arm = buildHelper("aarch64-apple-darwin");
    const intel = buildHelper("x86_64-apple-darwin");
    const universal = join(helperTargetDir, `${helperBin}-universal`);
    execFileSync("lipo", ["-create", "-output", universal, arm, intel], { stdio: "inherit" });
    stageHelper(universal);
  } else {
    stageHelper(buildHelper(target));
  }
} else {
  // Windows and Linux: the helper program, next to brigadierd once bundled.
  stage(buildHelper(target), helperBin, target);
}
