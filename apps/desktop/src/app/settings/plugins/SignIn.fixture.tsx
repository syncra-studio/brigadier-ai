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
let stops = 0;
let saved = false;
const catalog: CatalogView = { version: "fixture", rows: [{
  id: "server", kind: "server", name: "Test server", differentPerAgent: false,
  choice: { on: true, scope: "global" }, signIn: { type: "signedOut" },
  definitions: [{ agent: "claude", file: "/fixture/.mcp.json", plugin: null, project: null,
    native: "server", aliases: [], fingerprint: "fixture", transport: "http", endpoint: "https://fixture.test/mcp" }],
}] };
mockIPC((_command, payload) => {
  const request = (payload as { request: Request }).request;
  if (request.method === "getPluginCatalog") return { method: request.method, catalog: structuredClone(catalog) };
  if (request.method === "startPluginSignIn") {
    starts++;
    if (state.type === "cliFallback") state = { type: "cliFallback", agents: ["codex"], steps: [
      { agent: "claude", server: "server", state: { type: "alreadySignedIn" } },
      { agent: "codex", server: "server", state: { type: "inBrowser" } },
    ] };
    else state = { type: "signingIn", flowId: "fixture" };
  }
  if (request.method === "cancelPluginSignIn") { stops++; state = { type: saved ? "signedIn" : "signedOut" }; }
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
  const button = buttons(selector).find((candidate) => candidate.textContent === label || candidate.getAttribute("aria-label") === label);
  if (!button) throw new Error(`Missing button ${label}`);
  button.click();
};
const waiting = (selector: string) => buttons(selector).some((button) => button.textContent === "Waiting for browser…" && button.disabled && !!button.querySelector("svg"));
const row = '[data-slot="plugin-row"]';
const detail = '[data-slot="sign-in-status"]';
async function run() {
  await wait(() => text('[data-slot="row-line"]') === "Must sign in");
  const warning = document.querySelector('[data-slot="row-line"]')?.classList.contains("text-warning");
  click(row, "Sign in");
  await wait(() => waiting(row));
  const gridWaiting = waiting(row);
  const gridStayedOpen = !!document.querySelector('[data-slot="plugins-grid"]');
  click(row, "Cancel");
  await wait(() => stops === 1 && text('[data-slot="row-line"]') === "Must sign in");
  const canceled = text('[data-slot="row-line"]');
  click(row, "Sign in");
  await wait(() => waiting(row));
  click(row, "Open Test server");
  await wait(() => waiting(detail));
  const detailWaiting = waiting(detail);
  const beforeCallbackStarts = starts;
  const callback = performance.now();
  saved = true;
  state = { type: "signedIn" };
  await wait(() => text(`${detail} [role="status"]`) === "Signed in");
  const callbackMs = performance.now() - callback;
  const signedInButtons = buttons(detail).map((button) => button.textContent);
  click(detail, "Sign in again");
  await wait(() => waiting(detail) && starts === 3);
  const againWaiting = waiting(detail);
  click(detail, "Cancel");
  await wait(() => text(`${detail} [role="status"]`) === "Signed in" && stops === 2);
  const canceledAgainButtons = buttons(detail).map((button) => button.textContent);
  click(detail, "Sign in again");
  await wait(() => waiting(detail) && starts === 4);
  openSettings("plugins");
  await wait(() => waiting(row));
  state = { type: "signedIn" };
  await wait(() => !document.querySelector(`${row} [data-slot="sign-in-actions"]`));
  const grid = text('[data-slot="row-line"]');
  const gridLine = !!document.querySelector('[data-slot="row-line"]');
  const gridButtons = buttons(row).map((button) => button.textContent);
  saved = false;

  state = { type: "cliFallback", agents: ["codex"], steps: [] };
  await usePlugins.getState().reload();
  await wait(() => text('[data-slot="row-line"]').includes("Uses each CLI's sign-in"));
  const fallback = text('[data-slot="row-line"]');
  click(row, "Sign in");
  await wait(() => waiting(row) && text('[data-slot="row-line"]').includes("Codex: waiting for browser"));
  click(row, "Cancel");
  await wait(() => stops === 3 && text('[data-slot="row-line"]') === "Must sign in");

  state = { type: "cliSetupRequired", agents: ["claude", "codex"] };
  await usePlugins.getState().reload();
  await wait(() => text('[data-slot="row-line"]').includes("add it in that CLI"));
  click(row, "Open Test server");
  await wait(() => text(detail).includes("add it in that CLI"));
  const setup = text(detail);
  const setupButtons = buttons(detail).map((button) => button.textContent);
  const dialogs = document.querySelectorAll('[data-dialog="sign-in"]').length;
  const banners = document.querySelectorAll('[data-slot="banner"]').length;
  disconnect();
  return { warning, gridWaiting, gridStayedOpen, canceled, detailWaiting, beforeCallbackStarts,
    callbackMs, signedInButtons, againWaiting, canceledAgainButtons, grid, gridLine, gridButtons, fallback, setup, setupButtons, dialogs, banners };
}
void run().then((result) => { document.getElementById("result")!.textContent = JSON.stringify(result); }, (error: unknown) => {
  disconnect(); document.getElementById("result")!.textContent = JSON.stringify({ error: String(error) });
});
