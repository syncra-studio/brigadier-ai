// Run with: node --test apps/desktop/scripts/nested-swift.test.mjs
import assert from "node:assert/strict";
import { chmodSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, join, resolve } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import { nestedSwiftEnv } from "./nested-swift.mjs";

const repoRoot = resolve(fileURLToPath(new URL("../../..", import.meta.url)));
const wrapper = join(repoRoot, "tools/nested-swift");

/** Under tools/full-checks.sh the wrapper is on PATH already: the real swift is still the one found (#510). */
test("the real swift is found, never the wrapper, however often the wrapper is put first", () => {
  const root = mkdtempSync(join(tmpdir(), "brigadier-nested-swift-"));
  try {
    mkdirSync(join(root, "bin"));
    writeFileSync(join(root, "bin/swift"), "#!/bin/sh\n");
    chmodSync(join(root, "bin/swift"), 0o755);
    const once = nestedSwiftEnv(repoRoot, { PATH: `${join(root, "bin")}${delimiter}/usr/bin${delimiter}/bin` });
    assert.equal(once.BRIGADIER_SWIFT, join(root, "bin/swift"));
    assert.equal(once.PATH.split(delimiter)[0], wrapper);
    // As full-checks.sh puts it: its checkout's path may be /tmp's, a link to /private/tmp.
    const linked = wrapper.startsWith("/tmp/") ? `/private${wrapper}` : wrapper.replace(/^\/private\/tmp\//, "/tmp/");
    const twice = nestedSwiftEnv(repoRoot, { ...once, PATH: `${linked}${delimiter}${wrapper}/${delimiter}${once.PATH}` });
    assert.equal(twice.BRIGADIER_SWIFT, join(root, "bin/swift"));
    assert.deepEqual(twice.PATH.split(delimiter), once.PATH.split(delimiter));
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
