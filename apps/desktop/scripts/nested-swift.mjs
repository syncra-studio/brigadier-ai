// The environment the computer-use helper builds in (stage-sidecar.mjs). Its Swift bridges are
// SwiftPM packages, and SwiftPM compiles their manifests under sandbox-exec, which macOS refuses
// inside another sandbox (a Brigadier session's preview or command, #510). There,
// tools/nested-swift's `swift` builds with --disable-sandbox and the outer sandbox still holds.
//
// Run its test with: node --test apps/desktop/scripts/nested-swift.test.mjs

import { execFileSync, spawnSync } from "node:child_process";
import { realpathSync } from "node:fs";
import { delimiter, resolve } from "node:path";

/** Whether this process may apply a sandbox of its own (it isn't inside one that refuses it). */
export function canSandbox() {
  // The probe denies something: inside a sandbox that allows everything, an identical profile
  // still applies.
  const strict = '(version 1)(allow default)(deny file-write* (literal "/private/var/empty/nested-probe"))';
  return spawnSync("/usr/bin/sandbox-exec", ["-p", strict, "/usr/bin/true"]).status === 0;
}

/** `dir` with its links followed (/tmp is /private/tmp), as given when it doesn't exist. */
function real(dir) {
  try {
    return realpathSync(dir);
  } catch {
    return resolve(dir);
  }
}

/**
 * `env` with tools/nested-swift first on PATH and BRIGADIER_SWIFT the real swift. The real one
 * is looked up without the wrapper's folder: under tools/full-checks.sh the wrapper is on PATH
 * already, and taken for the real one it would exec itself for ever.
 */
export function nestedSwiftEnv(repoRoot, env) {
  const wrapper = resolve(repoRoot, "tools/nested-swift");
  const path = (env.PATH ?? "")
    .split(delimiter)
    .filter((dir) => dir && real(dir) !== real(wrapper))
    .join(delimiter);
  const swift = execFileSync("/bin/sh", ["-c", "command -v swift"], { encoding: "utf8", env: { ...env, PATH: path } }).trim();
  return { ...env, BRIGADIER_SWIFT: swift, PATH: `${wrapper}${delimiter}${path}` };
}
