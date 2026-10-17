import assert from "node:assert/strict";
import { test } from "node:test";
import { build, type Rolldown } from "vite";

/**
 * Libraries only some views need: each loads with its view. One in the startup chunk is code the
 * webview parses before the first paint (the diff cards' static highlighter import once added
 * 184 KB there, #77).
 */
const LAZY = ["shiki", "@shikijs", "mermaid", "cytoscape", "elkjs", "@xterm", "katex"];

test("the startup chunk holds none of the libraries that load with their views", { timeout: 120000 }, async () => {
  const output = (await build({ logLevel: "silent", build: { write: false } })) as Rolldown.RolldownOutput;
  const chunks = new Map(output.output.flatMap((item) => (item.type === "chunk" ? [[item.fileName, item] as const] : [])));
  const entry = [...chunks.values()].find((chunk) => chunk.isEntry && chunk.facadeModuleId?.endsWith("/index.html"));
  assert.ok(entry, `no entry chunk among ${[...chunks.keys()].join(", ")}`);
  // The entry and what it imports statically: all of it loads before the app's code runs.
  const startup = new Set<string>();
  const visit = (fileName: string) => {
    if (startup.has(fileName)) return;
    startup.add(fileName);
    for (const imported of chunks.get(fileName)?.imports ?? []) visit(imported);
  };
  visit(entry.fileName);
  const pulled = [...startup].flatMap((fileName) =>
    (chunks.get(fileName)?.moduleIds ?? []).filter((id) => LAZY.some((name) => id.includes(`/node_modules/${name}/`))),
  );
  assert.deepEqual(pulled, [], `pulled into the startup chunk: ${pulled.join(", ")}`);
});
