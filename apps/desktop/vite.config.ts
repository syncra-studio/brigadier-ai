import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { fileURLToPath, URL } from "node:url";
import { defineConfig } from "vite";

// Tauri sets TAURI_ENV_* for `tauri dev` / `tauri build`.
const host = process.env.TAURI_DEV_HOST;
const platform = process.env.TAURI_ENV_PLATFORM;

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      "@": fileURLToPath(new URL("./src", import.meta.url)),
    },
  },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
  envPrefix: ["VITE_", "TAURI_ENV_"],
  build: {
    // WKWebView on macOS 14+ is Safari 17; WebView2 is evergreen Chromium; WebKitGTK tracks Safari.
    target: platform === "windows" ? "chrome120" : "safari17",
    minify: !process.env.TAURI_ENV_DEBUG,
    sourcemap: Boolean(process.env.TAURI_ENV_DEBUG),
    // Served from inside the app bundle, not over a network: one main chunk loads faster than
    // several at cold start (measured by the smoke check). The Inspector is split out lazily.
    chunkSizeWarningLimit: 1024,
    rolldownOptions: {
      output: {
        // Left to itself the bundler splits what startup needs into dozens of chunks shared with
        // the lazy views, each a request the webview makes before the app's code runs.
        codeSplitting: { groups: [{ name: "index", tags: ["$initial"] }] },
      },
    },
  },
});
