#!/usr/bin/env node
// Moves imports of `@tauri-apps/api/*` in apps/desktop/src onto the seam, `@/ipc/native` and
// `@/ipc/nativeMocks`. Safe to run again, so a file merged in from main with the old imports gets
// the same treatment. Only rewrites the import statements of tests, fixtures and other mock users;
// production modules were moved by hand, and a production import left over fails the lint rule.
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const desktop = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../apps/desktop");
const files = execFileSync("git", ["grep", "-l", "@tauri-apps/api/", "--", "src"], { cwd: desktop, encoding: "utf8" })
  .split("\n")
  .filter((file) => file !== "" && !/^src\/ipc\/native(Mocks)?\.ts$/.test(file));

const rewrites = [
  [/(from |import\()"@tauri-apps\/api\/mocks"/g, '$1"@/ipc/nativeMocks"'],
  [/import \{ emit \} from "@tauri-apps\/api\/event"/g, 'import { emit } from "@/ipc/nativeMocks"'],
  [/import type \{ Channel \} from "@tauri-apps\/api\/core"/g, 'import type { Channel } from "@/ipc/native"'],
];

let changed = 0;
const left = [];
for (const file of files) {
  const full = path.join(desktop, file);
  const before = readFileSync(full, "utf8");
  // Text in a fixture's copy that only mentions a package isn't an import, so only import forms match.
  const after = rewrites.reduce((text, [pattern, to]) => text.replace(pattern, to), before);
  if (after !== before) {
    writeFileSync(full, after);
    changed += 1;
  }
  if (/(from |import\()"@tauri-apps\/api\//.test(after)) left.push(file);
}
console.log(`rewrote ${changed} of ${files.length} files`);
if (left.length > 0) console.log(`still importing Tauri directly (move these by hand):\n  ${left.join("\n  ")}`);
