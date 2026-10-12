import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const src = fileURLToPath(new URL("..", import.meta.url));
const tokens = readFileSync(new URL("tokens.css", import.meta.url), "utf8");

/** The app's source files, without tests and fixture pages. */
function sources(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) return entry.name === "fixtures" ? [] : sources(path);
    return /\.tsx?$/.test(entry.name) && !entry.name.endsWith(".test.ts") ? [path] : [];
  });
}

/** Widths every theme has: numbers are spacing steps, the rest are keywords. */
const BUILT_IN = /^(\d+(\.\d+)?|full|none|px|min|max|fit)$/;

// tokens.css resets `--container-*`, so a stock name it doesn't define (max-w-xl, say)
// makes no rule at all and the element spans its whole container.
test("every max-w-* the app uses names a width tokens.css defines", () => {
  const missing = new Set<string>();
  for (const file of sources(src)) {
    for (const [, name] of readFileSync(file, "utf8").matchAll(/(?<![\w-])max-w-([a-z0-9][a-z0-9-]*)/g)) {
      if (!name || BUILT_IN.test(name)) continue;
      if (!tokens.includes(`--container-${name}:`) && !tokens.includes(`--spacing-${name}:`)) {
        missing.add(`max-w-${name} (${file.slice(src.length)})`);
      }
    }
  }
  assert.deepEqual([...missing], []);
});
