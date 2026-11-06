#!/usr/bin/env node
// Builds the Electron main process and preload from TypeScript into dist-electron. A preload in a sandbox
// can require only "electron", so it's one CommonJS file. --production drops the dev control.
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "vite";

const root = fileURLToPath(new URL("..", import.meta.url));
const production = process.argv.includes("--production");
const bundle = (entry, name, format) => build({
  root,
  configFile: false,
  logLevel: "warn",
  // A packaged app carries no node_modules: what main needs goes in its bundle.
  ssr: { noExternal: true },
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
      // Bundled CommonJS (electron-updater's) reaches for these by `require`, which an ES module lacks.
      output: { format, ...(format === "es" ? { banner: 'import { createRequire as __createRequire } from "node:module";\nconst require = __createRequire(import.meta.url);' } : {}), entryFileNames: `[name].${format === "cjs" ? "cjs" : "mjs"}`, chunkFileNames: "[name].mjs" },
    },
  },
});
// A chunk left by an earlier build would ship: electron-builder packs what's here.
const out = path.join(root, "dist-electron");
for (const file of fs.existsSync(out) ? fs.readdirSync(out) : []) if (/\.(mjs|cjs)$/.test(file)) fs.rmSync(path.join(out, file));
await bundle("./main/index.ts", "main", "es");
await bundle("./preload.ts", "preload", "cjs");
await bundle("./guestPasskey.ts", "guestPasskey", "cjs");
