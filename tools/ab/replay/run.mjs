// A/B measurement harness: replays a recorded daemon event stream through the desktop app's own
// stores and renders the live request block with the real RequestBlock. See README.md.
// Usage: ARM=<arm-dir> [APP=<apps/desktop of the code to replay through>] node tools/ab/replay/run.mjs
// APP defaults to this checkout's apps/desktop. The harness is copied into APP/.ab-replay for the
// run (so its imports resolve against APP's packages) and removed after.
import { cpSync, mkdirSync, readdirSync, readFileSync, rmSync } from "node:fs";
import { createRequire } from "node:module";
import { join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = fileURLToPath(new URL(".", import.meta.url));
const app = resolve(process.env.APP || join(here, "../../../apps/desktop"));
const appRequire = createRequire(join(app, "package.json"));
const { default: react } = await import(pathToFileURL(appRequire.resolve("@vitejs/plugin-react")).href);
const { createServer } = await import(pathToFileURL(appRequire.resolve("vite")).href);
// parse5 is in the workspace only as a transitive dependency.
const pnpmStore = join(app, "../../node_modules/.pnpm");
const parse5Dir = readdirSync(pnpmStore).filter((d) => /^parse5@7\./.test(d)).sort().pop();
if (!parse5Dir) throw new Error(`no parse5@7 in ${pnpmStore}`);
const { parseFragment } = await import(
  pathToFileURL(join(pnpmStore, parse5Dir, "node_modules/parse5/dist/index.js")).href
);

const arm = process.env.ARM;
if (!arm) throw new Error("Set ARM=/path/to/arm");

// The replay clock: every Date.now() the app code reads is the replayed time. Set before any app
// module loads (module-level clocks such as use-activity-clock read it at import).
const realNow = Date.now.bind(Date);
globalThis.__replayNow = null;
Date.now = () => (globalThis.__replayNow ?? realNow());

// Minimal browser globals that app modules touch at import or render time.
const storage = new Map();
globalThis.localStorage = {
  getItem: (k) => (storage.has(k) ? storage.get(k) : null),
  setItem: (k, v) => storage.set(k, String(v)),
  removeItem: (k) => storage.delete(k),
  clear: () => storage.clear(),
};
const style = { getPropertyValue: () => "" };
globalThis.document ??= {
  documentElement: { dataset: {}, style: { setProperty() {}, removeProperty() {} }, classList: { add() {}, remove() {}, toggle() {} } },
  addEventListener() {},
  removeEventListener() {},
  hidden: false,
};
globalThis.window ??= globalThis;
globalThis.getComputedStyle ??= () => style;
globalThis.matchMedia ??= () => ({ matches: false, addEventListener() {}, removeEventListener() {} });

// React's server renderer reads a store's server snapshot (zustand: its *initial* state, others
// have none). The replay renders what the client would: every store's current state.
const React = appRequire("react");
const useSyncExternalStore = React.useSyncExternalStore;
React.useSyncExternalStore = (subscribe, getSnapshot) => useSyncExternalStore(subscribe, getSnapshot, getSnapshot);

const root = app;
const staged = join(app, ".ab-replay");
rmSync(staged, { recursive: true, force: true });
mkdirSync(staged);
for (const file of ["harness.tsx", "auiMock.tsx"]) cpSync(join(here, file), join(staged, file));
const auiMock = join(staged, "auiMock.tsx");

const server = await createServer({
  configFile: false,
  root,
  logLevel: "error",
  appType: "custom",
  cacheDir: join(arm, "replay-vite-cache"),
  server: { middlewareMode: true, hmr: false, watch: null },
  plugins: [
    react(),
    {
      // Only RequestBlock's assistant-ui imports are stood in for (it reads its block from the
      // assistant-ui message state); every other module gets the real package.
      name: "replay-aui-mock",
      enforce: "pre",
      transform(code, id) {
        if (!id.endsWith("/conversation/RequestBlock.tsx")) return null;
        const swapped = code.replace(/from "@assistant-ui\/react";/, `from ${JSON.stringify(auiMock)};`);
        if (swapped === code) throw new Error("RequestBlock no longer imports @assistant-ui/react as expected");
        return swapped;
      },
    },
  ],
  resolve: { alias: { "@": join(app, "src") } },
  ssr: { noExternal: ["@openai/apps-sdk-ui", "radix-ui", /^@radix-ui\//] },
});
// Module-level clocks read at import start at the arm's send time, not the wall clock.
globalThis.__replayNow = JSON.parse(readFileSync(`${arm}/start.json`, "utf8")).t0_ms;
try {
  const harness = await server.ssrLoadModule("/.ab-replay/harness.tsx");
  await harness.run(arm, parseFragment);
} finally {
  await server.close();
  rmSync(staged, { recursive: true, force: true });
}
