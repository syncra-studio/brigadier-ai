import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

// T3, hidden from both arms: Settings lists a page labelled Configuration. Read from the source:
// importing the pages pulls in icon modules that only a bundler resolves.
test("T3: Settings has a Configuration page", () => {
  const pages = readFileSync(new URL("./pages.ts", import.meta.url), "utf8");
  const entry = pages.match(/\{[^{}]*label:\s*"Configuration"[^{}]*\}/s);
  assert.ok(entry, "SETTINGS_PAGES has no page labelled Configuration");
  const rows = entry[0].match(/rows:\s*([^,\n]+)/)?.[1] ?? "";
  assert.ok(rows.trim().length > 0, "the Configuration page has no rows for Settings search");
});
