import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";

import { PENDING } from "./pending.ts";

const read = (path: string) => fs.readFileSync(new URL(path, import.meta.url), "utf8");

/** The bare command names of the Tauri build's handler list. */
function tauriCommands(): string[] {
  const list = /generate_handler!\[([\s\S]*?)\]/.exec(read("../../src-tauri/src/main.rs"))?.[1] ?? "";
  return list.split(",").map((each) => each.trim().split("::").pop() ?? "").filter(Boolean);
}

/** Names defined as `name: ` in a commands table, in main or the page. */
function defined(source: string): Set<string> {
  return new Set([...source.matchAll(/^\s+([a-z_]+): /gm)].map((match) => match[1] as string));
}

test("every Tauri command is answered by Electron or owned by an area, and never both", () => {
  const answered = defined(read("../renderer/browserCommands.ts"));
  const pending = Object.values(PENDING).flat();
  for (const file of fs.readdirSync(new URL(".", import.meta.url))) {
    if (file.endsWith(".ts") && !file.endsWith(".test.ts") && file !== "pending.ts") {
      for (const name of defined(read(`./${file}`))) answered.add(name);
    }
  }
  assert.deepEqual(tauriCommands().filter((name) => !answered.has(name) && !pending.includes(name)), [], "nobody owns these");
  assert.deepEqual(pending.filter((name) => answered.has(name)), [], "answered, so no longer pending");
  assert.equal(new Set(pending).size, pending.length);
});
