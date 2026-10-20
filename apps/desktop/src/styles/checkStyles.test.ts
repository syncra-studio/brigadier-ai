/* oxlint-disable brigadier/no-raw-design-values -- the style check's samples are raw on purpose. */
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const script = fileURLToPath(new URL("../../scripts/check-styles.mjs", import.meta.url));

/** Runs the style check over a tree of `files`, with `allow` as its allowlist. */
function check(files: Record<string, string>, allow = "") {
  const dir = mkdtempSync(join(tmpdir(), "check-styles-"));
  try {
    for (const [name, text] of Object.entries(files)) {
      mkdirSync(join(dir, "src", name, ".."), { recursive: true });
      writeFileSync(join(dir, "src", name), text);
    }
    writeFileSync(join(dir, "allow"), allow);
    const run = spawnSync(process.execPath, [script, join(dir, "src"), join(dir, "allow")], { encoding: "utf8" });
    return { ok: run.status === 0, errors: run.stderr.trim().split("\n").filter(Boolean) };
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

test("the style check passes the app's own tree", () => {
  const run = spawnSync(process.execPath, [script], { encoding: "utf8" });
  assert.equal(run.stderr, "");
  assert.equal(run.status, 0);
});

test("the style check takes tokens and computed values, and refuses raw ones", () => {
  const good = check({
    "styles/tokens.css": ":root { --x: #fff; --y: 12px; }",
    "a.css": ".a { padding: calc(var(--spacing) * 2); border: 1px solid var(--border); width: 100%; background: color-mix(in oklab, var(--foreground) 5%, transparent); margin: 0; }",
    "b.tsx": 'export const B = ({ w }: { w: number }) => <div className="max-h-(--radix-popover-content-available-height)" style={{ width: `${w}px`, flexBasis: 0, top: w }} />;',
    "components/ui/c.tsx": 'import { Dialog } from "radix-ui";',
  });
  assert.deepEqual(good, { ok: true, errors: [] });

  const bad = check({
    "a.css": "/* 12px in a comment is fine */ .a { color: #123456; padding: 12px; background: oklch(0.5 0 0); }",
    "b.tsx": 'import { Popover } from "radix-ui";\nexport const B = () => <div style={{ color: "red", zIndex: 3 }} />;',
  });
  assert.equal(bad.ok, false);
  assert.deepEqual(bad.errors.map((error) => error.split(" is ")[0]), [
    'src/a.css: "#123456"',
    'src/a.css: "oklch("',
    'src/a.css: "12px"',
    'src/b.tsx: "radix-ui"',
    'src/b.tsx: "style.color"',
    'src/b.tsx: "style.zIndex"',
  ]);
});

test("the allowlist lets a file keep the values it names, and an entry no longer needed fails", () => {
  const files = { "a.css": ".a { padding: 12px; }" };
  assert.equal(check(files, "a.css 12px  # reason").ok, true);
  assert.deepEqual(check(files, "a.css 12px 3px  # reason").errors, [
    'check-styles.allow: src/a.css no longer has "3px"; take it off the list',
  ]);
  assert.equal(check(files, "a.css 12px").ok, false);
});
