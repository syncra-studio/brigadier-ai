// Serves the real front end (apps/desktop/src, its own index.html) to Electron, with one change: the
// page's script is electron/renderer/entry.ts, which installs the shell's IPC and then runs src/main.tsx.
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { fileURLToPath, URL } from "node:url";
import { defineConfig } from "vite";

export default defineConfig({
  root: fileURLToPath(new URL("..", import.meta.url)),
  plugins: [
    react(),
    tailwindcss(),
    {
      // The dev client reloads the page when its socket comes back, and a reload destroys every tab. In a
      // gate run it says so instead. Production has no such client.
      name: "brigadier-no-client-reload",
      enforce: "pre",
      apply: () => process.env.BRIGADIER_NO_HMR === "1",
      transform: (code, id) => (id.includes("vite/dist/client/client.mjs") ? code.replaceAll("location.reload()", 'console.warn("[vite] client wanted to reload the page")') : null),
    },
    {
      name: "brigadier-electron-entry",
      transformIndexHtml: {
        order: "pre",
        handler: (html) => html.replace(/src="\/src\/main\.tsx"/, 'src="/electron/renderer/entry.ts"'),
      },
    },
  ],
  resolve: { alias: { "@": fileURLToPath(new URL("../src", import.meta.url)) } },
  clearScreen: false,
  cacheDir: fileURLToPath(new URL("../../../target/browser-check/vite", import.meta.url)),
  server: {
    port: 1431,
    strictPort: true,
    host: "127.0.0.1",
    // A gate run needs the page's production lifetime: no client that can reload it, nothing watched.
    ...(process.env.BRIGADIER_NO_HMR === "1" ? { hmr: false, watch: null } : { watch: { ignored: ["**/src-tauri/**"] } }),
  },
  envPrefix: ["VITE_", "TAURI_ENV_"],
  // The production page, as src/ builds for Tauri (vite.config.ts): one main chunk, served from inside the app.
  build: {
    outDir: "dist-electron/page",
    target: "chrome140",
    chunkSizeWarningLimit: 1024,
    rolldownOptions: {
      treeshake: { moduleSideEffects: (id) => (/\/src\/components\/(?!icons\/).+\.tsx?$/.test(id) ? false : undefined) },
      output: { codeSplitting: { groups: [{ name: "index", tags: ["$initial"] }] } },
    },
  },
});
