import { createRoot } from "react-dom/client";
import { mockIPC } from "@tauri-apps/api/mocks";
import { TooltipProvider } from "@/components/overlays";
import type { CatalogRow, CatalogView, PluginChanges, Request } from "@/ipc/generated";
import { useApp } from "@/state/store";
import { connectPlugins } from "./adapter";
import { livePlugins } from "./live";
import { PluginsPage } from "./PluginsPage";

const row = (id: string, kind: "server" | "skill", name: string, detail: string): CatalogRow => ({
  id, kind, name, differentPerAgent: false, choice: { on: true, scope: "global" }, signIn: { type: "notRequired" },
  definitions: [{ agent: "claude", file: kind === "skill" ? `${detail}/SKILL.md` : "/fixture/.mcp.json", plugin: null, project: null, native: name, aliases: [], fingerprint: id,
    transport: kind === "server" ? "http" : null, endpoint: kind === "server" ? detail : null }],
});
let rows = [row("linear", "server", "linear", "https://mcp.linear.app/mcp"), row("review", "skill", "code-review", "/home/.claude/skills/code-review")];
let changes: PluginChanges = {
  found: [
    { key: "sentry", kind: "server", name: "sentry", detail: "https://mcp.sentry.dev/mcp", agents: ["opencode"] },
    { key: "pdf-tools", kind: "skill", name: "pdf-tools", detail: "/home/.claude/skills/pdf-tools", agents: ["codex", "claude"] },
    { key: "changelog", kind: "skill", name: "changelog-writer", detail: "/home/.codex/skills/changelog-writer", agents: ["codex"] },
  ],
  gone: [{ key: "skill:old-deploy", kind: "skill", name: "old-deploy" }, { key: "server:notion", kind: "server", name: "notion" }],
};
let missing: CatalogView["missing"];
let hidden: CatalogView["hidden"];
const requests: Request[] = [];
mockIPC((_command, payload) => {
  const request = (payload as { request: Request }).request;
  if (request.method === "getPluginCatalog") return { method: request.method, catalog: { version: JSON.stringify([rows.length, missing, hidden]), rows: structuredClone(rows), ...(missing ? { missing } : {}), ...(hidden ? { hidden } : {}) } };
  requests.push(request);
  if (request.method === "checkPluginChanges") return { method: request.method, changes: structuredClone(changes) };
  if (request.method === "applyPluginChanges") {
    for (const item of changes.found) if (request.add.includes(item.key)) rows = [...rows, row(item.key, item.kind, item.name, item.detail)];
    hidden = changes.found.filter((item) => request.decline.includes(item.key)).map(({ key, kind, name, detail }) => ({ key, kind, name, detail }));
    missing = [...(missing ?? []).filter((item) => !request.remove.includes(item.key)), ...changes.gone.filter((item) => request.keep.includes(item.key))];
    changes = { found: [], gone: [] };
    return { method: request.method };
  }
  if (request.method === "restorePlugin") {
    const item = hidden?.find((entry) => entry.key === request.key);
    if (item) rows = [...rows, row(item.key, item.kind, item.name, item.detail)];
    hidden = hidden?.filter((entry) => entry.key !== request.key);
    return { method: request.method };
  }
  throw new Error(`Unexpected ${request.method}`);
});
useApp.setState({ selection: { type: "settings", page: "plugins" } });
const disconnect = connectPlugins(livePlugins());
createRoot(document.getElementById("root")!).render(<TooltipProvider><PluginsPage /></TooltipProvider>);
const dialog = '[data-dialog="refresh"]';
const text = (selector: string) => document.querySelector(selector)?.textContent ?? "";
const named = (label: string) => [...document.querySelectorAll<HTMLElement>("button")].find((button) => button.textContent === label || button.getAttribute("aria-label") === label);
const click = (label: string) => {
  const button = named(label);
  if (!button) throw new Error(`Missing ${label}`);
  button.click();
};
async function wait(check: () => boolean) {
  const until = performance.now() + 4000;
  while (!check()) { if (performance.now() > until) throw new Error(`Timed out: ${document.body.textContent}`); await new Promise((resolve) => setTimeout(resolve, 20)); }
}
const tab = (label: string) => [...document.querySelectorAll<HTMLElement>("[role=tab]")].find((entry) => entry.textContent?.startsWith(label))!.click();
async function run() {
  await wait(() => !!named("Refresh"));
  await new Promise((resolve) => setTimeout(resolve, 200));
  const askedOnOpen = requests.length;
  click("Refresh");
  await wait(() => !!document.querySelector(dialog));
  const prompt = text(dialog);
  click("Not now");
  await wait(() => !document.querySelector(dialog));
  const afterNotNow = requests.map((request) => request.method);
  click("Refresh");
  await wait(() => !!document.querySelector(dialog));
  click("Add sentry");
  click("Add pdf-tools");
  [...document.querySelectorAll<HTMLElement>('[aria-label="What to do with notion"] button')].find((button) => button.textContent === "Remove")!.click();
  await wait(() => !!named("Apply"));
  click("Apply");
  await wait(() => !document.querySelector(dialog));
  tab("Skills");
  await wait(() => !!document.querySelector('[data-slot="plugin-missing"]'));
  const missingCard = text('[data-slot="plugin-missing"]');
  click("Refresh");
  await wait(() => !!document.querySelector('[data-slot="nothing-changed"]'));
  const nothing = text('[data-slot="nothing-changed"]');
  click("Add");
  await wait(() => !!named("Bring back…"));
  click("Bring back…");
  await wait(() => !!named("Bring back changelog-writer"));
  click("Bring back changelog-writer");
  await wait(() => rows.some((entry) => entry.id === "changelog") && !named("Bring back changelog-writer"));
  disconnect();
  return { askedOnOpen, prompt, afterNotNow, missingCard, nothing, requests };
}
void run().then((result) => { document.getElementById("result")!.textContent = JSON.stringify(result); }, (error: unknown) => {
  disconnect(); document.getElementById("result")!.textContent = JSON.stringify({ error: String(error) });
});
