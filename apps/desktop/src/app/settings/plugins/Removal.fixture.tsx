import { createRoot } from "react-dom/client";
import { mockIPC } from "@tauri-apps/api/mocks";
import { TooltipProvider } from "@/components/overlays";
import type { CatalogRow, Request } from "@/ipc/generated";
import { select } from "@/state/actions";
import { useApp } from "@/state/store";
import { connectPlugins } from "./adapter";
import { livePlugins } from "./live";
import { PluginsPage } from "./PluginsPage";

const row = (kind: "server" | "skill"): CatalogRow => ({
  id: kind, kind, name: `Native ${kind}`, differentPerAgent: false,
  choice: { on: true, scope: "global" }, signIn: { type: "notRequired" },
  definitions: ["claude", "codex"].map((agent) => ({ agent: agent as "claude" | "codex", file: `/fixture/.${agent}/${kind}`, plugin: null,
    project: null, native: kind, aliases: [], fingerprint: kind, transport: kind === "server" ? "stdio" : null, endpoint: kind === "server" ? "native-mcp" : null })),
});
let rows = [row("server"), row("skill")];
let fail = true;
const removes: string[] = [];
mockIPC((_command, payload) => {
  const request = (payload as { request: Request }).request;
  if (request.method === "getPluginCatalog") return { method: request.method, catalog: { version: String(rows.length), rows: structuredClone(rows) } };
  if (request.method === "removePlugin") {
    removes.push(request.key);
    if (fail) { fail = false; throw new Error("Hide failed"); }
    rows = rows.filter((entry) => entry.id !== request.key);
    return { method: request.method, job: "" };
  }
  throw new Error(`Unexpected ${request.method}`);
});
useApp.setState({ selection: { type: "settings", page: "plugins", detail: { plugin: "server" } } });
const disconnect = connectPlugins(livePlugins());
createRoot(document.getElementById("root")!).render(<TooltipProvider><PluginsPage /></TooltipProvider>);
const dialog = '[data-dialog="remove"]';
const text = (selector: string) => document.querySelector(selector)?.textContent ?? "";
const findButton = (label: string, scope = "#root") => [...document.querySelectorAll<HTMLButtonElement>(`${scope} button`)].find((candidate) => candidate.textContent === label);
const click = (label: string, scope?: string) => {
  const button = findButton(label, scope);
  if (!button) throw new Error(`Missing ${label}`);
  button.click();
};
async function wait(check: () => boolean) {
  const until = performance.now() + 4000;
  while (!check()) { if (performance.now() > until) throw new Error(`Timed out: ${document.body.textContent}`); await new Promise((resolve) => setTimeout(resolve, 20)); }
}
async function run() {
  await wait(() => !!findButton("Remove Native server"));
  click("Remove Native server");
  await wait(() => !!document.querySelector(dialog));
  const confirmation = text(dialog);
  click("Cancel", dialog);
  await wait(() => !document.querySelector(dialog));
  const canceledCalls = removes.length;
  click("Remove Native server");
  await wait(() => !!document.querySelector(dialog));
  click("Remove", dialog);
  await wait(() => text(dialog).includes("Hide failed") && !findButton("Remove", dialog)?.disabled);
  const errorVisible = text(dialog).includes("Hide failed");
  const retainedAfterFailure = rows.some((entry) => entry.id === "server");
  click("Remove", dialog);
  await wait(() => !document.querySelector(dialog) && useApp.getState().selection.type === "settings" && text("h1") === "Plugins");
  const hiddenServer = !rows.some((entry) => entry.id === "server");
  select({ type: "settings", page: "plugins", detail: { plugin: "skill" } });
  await wait(() => !!findButton("Remove Native skill"));
  click("Remove Native skill");
  await wait(() => !!document.querySelector(dialog));
  const skillConfirmation = text(dialog);
  click("Remove", dialog);
  await wait(() => !document.querySelector(dialog) && rows.length === 0);
  const noRestore = !document.body.textContent?.includes("Restore") && !document.body.textContent?.includes("Removed (");
  disconnect();
  return { confirmation, canceledCalls, errorVisible, retainedAfterFailure, hiddenServer, skillConfirmation, removes, noRestore };
}
void run().then((result) => { document.getElementById("result")!.textContent = JSON.stringify(result); }, (error: unknown) => {
  disconnect(); document.getElementById("result")!.textContent = JSON.stringify({ error: String(error) });
});
