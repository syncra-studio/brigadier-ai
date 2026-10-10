// Builds brigadierd and stages it where Tauri's `externalBin` expects it:
// src-tauri/binaries/brigadierd-<target-triple>[.exe].
//
// Runs automatically from beforeDevCommand / beforeBuildCommand. The target comes from
// TAURI_ENV_TARGET_TRIPLE (set by the Tauri CLI), then `--target <triple>`, then the host.
// For `universal-apple-darwin` both architectures are built and merged with `lipo`; the
// per-architecture binaries are staged too because each architecture's build script
// copies its own sidecar.
//
// On macOS it also assembles the computer-use helper, src-tauri/binaries/Brigadier Computer
// Use.app, which bundle.macOS.files puts in Contents/Helpers. Its bundle id is the build's own
// (`--helper-id`, from the dev config for dev builds) so the system's permissions name it, not
// Brigadier or a terminal. It is signed ad hoc, or with APPLE_SIGNING_IDENTITY when a release
// build sets it.

import { execFileSync } from "node:child_process";
import { copyFileSync, chmodSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "../../..");
const binariesDir = resolve(here, "../src-tauri/binaries");
// Where cargo puts its output: CARGO_TARGET_DIR (relative to the repo, where cargo runs) or target/.
const targetDir = resolve(repoRoot, process.env.CARGO_TARGET_DIR || "target");
const name = "brigadierd";

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

/** Builds `bin` of `pkg` for `triple` and returns the path of the produced binary. */
function build(triple, pkg = "brigadier-daemon", bin = name) {
  const cargoArgs = ["build", "--locked", "-p", pkg, "--bin", bin];
  if (release) cargoArgs.push("--release");
  // Building for the host without --target shares its build cache with other cargo commands.
  const crossTarget = triple !== host;
  if (crossTarget) cargoArgs.push("--target", triple);
  run("cargo", cargoArgs);
  const exe = triple.includes("windows") ? ".exe" : "";
  return join(targetDir, crossTarget ? triple : "", profile, `${bin}${exe}`);
}

function stage(source, triple) {
  const exe = triple.includes("windows") ? ".exe" : "";
  const destination = join(binariesDir, `${name}-${triple}${exe}`);
  copyFileSync(source, destination);
  chmodSync(destination, 0o755);
  console.log(`staged ${destination}`);
  return destination;
}

mkdirSync(binariesDir, { recursive: true });

if (target === "universal-apple-darwin") {
  const arm = stage(build("aarch64-apple-darwin"), "aarch64-apple-darwin");
  const intel = stage(build("x86_64-apple-darwin"), "x86_64-apple-darwin");
  const universal = join(binariesDir, `${name}-universal-apple-darwin`);
  execFileSync("lipo", ["-create", "-output", universal, arm, intel], { stdio: "inherit" });
  console.log(`staged ${universal}`);
} else {
  stage(build(target), target);
}

const helperName = "Brigadier Computer Use";
const helperBin = "brigadier-computer";

function plist(entries) {
  const body = Object.entries(entries)
    .map(([key, value]) => {
      const v = typeof value === "boolean" ? `<${value}/>` : `<string>${value}</string>`;
      return `  <key>${key}</key>\n  ${v}`;
    })
    .join("\n");
  return `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
${body}
</dict>
</plist>
`;
}

/** Assembles and signs the computer-use helper bundle around `binary`. */
function stageHelper(binary) {
  const id = flag("--helper-id") || "ai.brigadier.computer-use";
  const version = JSON.parse(readFileSync(resolve(here, "../package.json"), "utf8")).version;
  const app = join(binariesDir, `${helperName}.app`);
  rmSync(app, { recursive: true, force: true });
  mkdirSync(join(app, "Contents/MacOS"), { recursive: true });
  mkdirSync(join(app, "Contents/Resources"), { recursive: true });
  copyFileSync(binary, join(app, "Contents/MacOS", helperBin));
  chmodSync(join(app, "Contents/MacOS", helperBin), 0o755);
  copyFileSync(resolve(here, "../src-tauri/icons/icon.icns"), join(app, "Contents/Resources/AppIcon.icns"));
  writeFileSync(
    join(app, "Contents/Info.plist"),
    plist({
      CFBundleDevelopmentRegion: "en",
      CFBundleExecutable: helperBin,
      CFBundleIconFile: "AppIcon",
      CFBundleIdentifier: id,
      CFBundleName: helperName,
      CFBundleDisplayName: helperName,
      CFBundlePackageType: "APPL",
      CFBundleShortVersionString: version,
      CFBundleVersion: version,
      LSMinimumSystemVersion: "14.0",
      LSUIElement: true,
      NSHighResolutionCapable: true,
    }),
  );
  const identity = process.env.APPLE_SIGNING_IDENTITY || "-";
  execFileSync(
    "codesign",
    ["--force", "--sign", identity, "--identifier", id, "--options", "runtime", "--timestamp=none", app],
    { stdio: "inherit" },
  );
  console.log(`staged ${app} (${id})`);
}

if (target.includes("apple-darwin")) {
  if (target === "universal-apple-darwin") {
    const arm = build("aarch64-apple-darwin", "brigadier-computer", helperBin);
    const intel = build("x86_64-apple-darwin", "brigadier-computer", helperBin);
    const universal = join(targetDir, `${helperBin}-universal`);
    execFileSync("lipo", ["-create", "-output", universal, arm, intel], { stdio: "inherit" });
    stageHelper(universal);
  } else {
    stageHelper(build(target, "brigadier-computer", helperBin));
  }
}
