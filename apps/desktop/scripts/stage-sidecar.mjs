// Builds brigadierd and stages it where Tauri's `externalBin` expects it:
// src-tauri/binaries/brigadierd-<target-triple>[.exe].
//
// Runs automatically from beforeDevCommand / beforeBuildCommand. The target comes from
// TAURI_ENV_TARGET_TRIPLE (set by the Tauri CLI), then `--target <triple>`, then the host.
// For `universal-apple-darwin` both architectures are built and merged with `lipo`; the
// per-architecture binaries are staged too because each architecture's build script
// copies its own sidecar.

import { execFileSync } from "node:child_process";
import { copyFileSync, chmodSync, mkdirSync } from "node:fs";
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

/** Builds the daemon for `triple` and returns the path of the produced binary. */
function build(triple) {
  const cargoArgs = ["build", "--locked", "-p", "brigadier-daemon", "--bin", name];
  if (release) cargoArgs.push("--release");
  // Building for the host without --target shares its build cache with other cargo commands.
  const crossTarget = triple !== host;
  if (crossTarget) cargoArgs.push("--target", triple);
  run("cargo", cargoArgs);
  const exe = triple.includes("windows") ? ".exe" : "";
  return join(targetDir, crossTarget ? triple : "", profile, `${name}${exe}`);
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
