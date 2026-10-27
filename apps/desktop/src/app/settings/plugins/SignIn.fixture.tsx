import { createRoot } from "react-dom/client";
import { mockIPC } from "@tauri-apps/api/mocks";
import { TooltipProvider } from "@/components/overlays";
import type { CatalogSignIn, CatalogView, Request } from "@/ipc/generated";
import { openSettings } from "@/state/actions";
import { useApp } from "@/state/store";
import { connectPlugins, usePlugins } from "./adapter";
import { livePlugins } from "./live";
import { PluginsPage } from "./PluginsPage";

let state: CatalogSignIn = { type: "signedOut" };
let starts = 0;
let reads = 0;
const catalog: CatalogView = { version: "fixture", rows: [{
  id: "server", kind: "server", name: "Test server", differentPerAgent: false,
  choice: { on: true, scope: "global" }, signIn: { type: "signedOut" },
  definitions: [{ agent: "claude", file: "/fixture/.mcp.json", plugin: null, project: null,
    native: "server", aliases: [], fingerprint: "fixture", transport: "http", endpoint: "https://fixture.test/mcp" }],
}] };
mockIPC((_command, payload) => {
  const request = (payload as { request: Request }).request;
  if (request.method === "getPluginCatalog") { reads++; return { method: request.method, catalog: structuredClone(catalog) }; }
  if (request.method === "startPluginSignIn") {
    starts++;
    if (state.type !== "signedIn") state = { type: "signingIn", flowId: "fixture" };
  }
  return { method: request.method, state };
});
useApp.setState({ selection: { type: "settings", page: "plugins" } });
const disconnect = connectPlugins(livePlugins());
createRoot(document.getElementById("root")!).render(<TooltipProvider><PluginsPage /></TooltipProvider>);
const pause = () => new Promise<void>((resolve) => setTimeout(resolve, 20));
async function wait(check: () => boolean) {
  const until = performance.now() + 4000;
  while (!check()) { if (performance.now() > until) throw new Error(`Timed out: ${document.body.textContent}`); await pause(); }
}
const text = (selector: string) => document.querySelector(selector)?.textContent ?? "";
const buttons = (selector: string) => [...document.querySelectorAll<HTMLButtonElement>(`${selector} button`)];
const click = (selector: string, label: string) => {
  const button = buttons(selector).find((button) => button.textContent === label || button.getAttribute("aria-label") === label);
  if (!button) throw new Error(`Missing button ${label}`);
  button.click();
};
async function run() {
  await wait(() => text('[data-slot="row-line"]') === "Sign in");
  click("#root", "Open Test server");
  await wait(() => !!buttons('[data-slot="banners"]').length);
  click('[data-slot="banners"]', "Sign in");
  await wait(() => text('[data-slot="sign-in-state"]') === "Waiting for your browser…");
  const waitingDisabled = buttons('[data-dialog="sign-in"]').find((button) => button.textContent === "Waiting for your browser…")?.disabled;
  click('[data-dialog="sign-in"]', "Done");
  await wait(() => !document.querySelector('[data-dialog="sign-in"]'));
  click('[data-slot="banners"]', "Waiting for browser…");
  await wait(() => text('[data-slot="sign-in-state"]') === "Waiting for your browser…");
  const midFlowStarts = starts;
  const callback = performance.now();
  state = { type: "signedIn" };
  await wait(() => text('[data-slot="sign-in-state"]') === "Signed in");
  const callbackMs = performance.now() - callback;
  const signedInButtons = buttons('[data-dialog="sign-in"]').map((button) => button.textContent);
  click('[data-dialog="sign-in"]', "Done");
  await wait(() => !document.querySelector('[data-dialog="sign-in"]'));
  const detailButtons = buttons('[data-slot="banners"]').map((button) => button.textContent);
  openSettings("plugins");
  await wait(() => text('[data-slot="row-line"]') === "Signed in");
  const grid = text('[data-slot="row-line"]');

  // Reproduce an already signed-in backend behind a stale row.
  usePlugins.setState(({ view }) => ({ view: { ...view, items: view.items.map((item) => ({ ...item, signIn: { type: "signedOut" as const } })) } }));
  await wait(() => text('[data-slot="row-line"]') === "Sign in");
  click("#root", "Open Test server");
  await wait(() => !!buttons('[data-slot="banners"]').length);
  click('[data-slot="banners"]', "Sign in");
  await wait(() => text('[data-slot="sign-in-state"]') === "Signed in");
  const staleReply = text('[data-slot="sign-in-state"]');
  disconnect();
  return { waitingDisabled, midFlowStarts, callbackMs, signedInButtons, detailButtons, grid, staleReply, reads };
}
void run().then((result) => { document.getElementById("result")!.textContent = JSON.stringify(result); }, (error: unknown) => {
  disconnect(); document.getElementById("result")!.textContent = JSON.stringify({ error: String(error) });
});
