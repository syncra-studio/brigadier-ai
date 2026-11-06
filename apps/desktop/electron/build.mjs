#!/usr/bin/env node
// Builds the Electron main process and preload from TypeScript into dist-electron. A preload in a sandbox
// can require only "electron", so it's one CommonJS file. --production drops the dev control.
import { fileURLToPath } from "node:url";
import { build } from "vite";

const root = fileURLToPath(new URL("..", import.meta.url));
const production = process.argv.includes("--production");
const bundle = (entry, name, format) => build({
  root,
  configFile: false,
  logLevel: "warn",
  define: { BRIGADIER_PRODUCTION: String(production) },
  build: {
    ssr: true,
    target: "node22",
    outDir: "dist-electron",
    emptyOutDir: false,
    minify: false,
    rolldownOptions: {
      input: { [name]: fileURLToPath(new URL(entry, import.meta.url)) },
      external: ["electron", /^node:/],
      output: { format, entryFileNames: `[name].${format === "cjs" ? "cjs" : "mjs"}`, chunkFileNames: "[name].mjs" },
    },
  },
});
await bundle("./main/index.ts", "main", "es");
await bundle("./preload.ts", "preload", "cjs");
