// Run with: node --test tools/full-checks.test.mjs
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmodSync, cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

for (const [sandboxed, fail] of [[false, false], [true, false], [true, true]]) {
  test(`full checks ${sandboxed ? "inside" : "outside"} a sandbox${fail ? " with a failed build" : ""}`, { skip: process.platform !== "darwin" }, () => {
    const root = mkdtempSync(join(tmpdir(), "brigadier-full-checks-"));
    try {
      for (const dir of ["tools", "home/.cargo/bin", "bin", "node_modules", "apps/desktop/node_modules/vite", "helpers/computer-use"]) {
        mkdirSync(join(root, dir), { recursive: true });
      }
      cpSync(new URL("full-checks.sh", import.meta.url), join(root, "tools/full-checks.sh"));
      writeFileSync(join(root, "node_modules/.modules.yaml"), "");
      const executable = (path, body) => {
        writeFileSync(join(root, path), `#!/bin/sh\n${body}\n`);
        chmodSync(join(root, path), 0o755);
      };
      // The build scripts use `swift build`; later helper checks must see the wrapper too.
      executable("home/.cargo/bin/cargo", 'if [ "$1" = clippy ]; then swift build --package-path "helper with spaces"; fi');
      executable("bin/pnpm", 'case "$*" in *stage-sidecar*) swift build --package-path "bridge with spaces"; swift --version; if [ "$FAIL_STAGE" = 1 ]; then exit 23; fi ;; esac');
      executable("bin/ps", "echo 10");
      executable("bin/swift", 'printf "%s\\n" "$@" >> "$SWIFT_LOG"');
      const env = {
        ...process.env,
        HOME: join(root, "home"),
        PATH: `${join(root, "bin")}:${process.env.PATH}`,
        TMPDIR: root,
        CARGO_TARGET_DIR: sandboxed ? "target" : join(root, "target"),
        FAIL_STAGE: fail ? "1" : "0",
        SWIFT_LOG: join(root, "swift.log"),
      };
      assert.equal(spawnSync("git", ["init", "-q", root], { env }).status, 0);
      const command = sandboxed ? "/usr/bin/sandbox-exec" : "/bin/bash";
      const source = readFileSync(new URL("../crates/sandbox/src/macos.rs", import.meta.url), "utf8");
      const base = source.match(/const SEATBELT_BASE: &str = r#"([\s\S]*?)"#;/)[1];
      const args = sandboxed ? ["-p", `${base}(allow file-write*)`, "/bin/bash", "tools/full-checks.sh"] : ["tools/full-checks.sh"];
      const result = spawnSync(command, args, { cwd: root, env, encoding: "utf8" });
      assert.equal(result.status, fail ? 23 : 0, result.stdout + result.stderr);
      const lines = readFileSync(env.SWIFT_LOG, "utf8").trim().split("\n");
      const build = (path) => ["build", ...(sandboxed ? ["--disable-sandbox"] : []), "--package-path", path];
      assert.deepEqual(lines, [...build("bridge with spaces"), "--version", ...(fail ? [] : [...build("helper with spaces"), ...build("helper with spaces")])]);
      assert.equal(existsSync(join(root, "target")) && readdirSync(join(root, "target")).some((name) => name.startsWith("full-checks-swift.")), false);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
}
