import { createRoot } from "react-dom/client";
import { mockIPC } from "@tauri-apps/api/mocks";
import { TooltipProvider } from "@/components/overlays";
import type { CatalogRow, ProjectPrefs, Request } from "@/ipc/generated";
import { useApp } from "@/state/store";
import { connectPlugins } from "./adapter";
import { livePlugins } from "./live";
import { PluginsPage } from "./PluginsPage";

const adds: Array<Extract<Request, { method: "addPlugin" }>> = [];
const imports: Array<Extract<Request, { method: "importPlugin" }>> = [];
const offer = (id: string, file: string): CatalogRow => ({ id, name: id, kind: "server", differentPerAgent: false,
  choice: { on: true, scope: "global" }, signIn: { type: "notRequired" }, definitions: [{ agent: null, file, plugin: null,
    project: null, native: id, aliases: [], fingerprint: id, transport: "http", endpoint: "https://old.test/mcp" }] });
const picked = "---\nname: picked\ndescription: Folder skill\n---\nOld instructions\n";
let folders = 0;
mockIPC((command, payload) => {
  if (command === "pick_folder") { folders++; return "/fixture/chosen"; }
  const request = (payload as { request: Request }).request;
  if (request.method === "getPluginCatalog") return { method: request.method, catalog: { version: "one", rows: [offer("External", "/fixture/desktop.json"), offer("Cursor", "/fixture/cursor/mcp.json")] } };
  if (request.method === "getPluginItem") return { method: request.method, item: { key: request.key, kind: "server", skills: [], server: { type: "http", url: "https://old.test/mcp", headerNames: ["Authorization"] } } };
  if (request.method === "checkPluginAdd") {
    let name = request.name;
    let kind: "skill" | "server" = "server";
    let contents: string | undefined;
    let secrets: string[] = [];
    if (request.text === "/fixture/chosen") { name = "picked"; kind = "skill"; contents = picked; }
    else if (request.text.startsWith("---")) { name = request.text.match(/^name: (.+)$/m)?.[1] ?? null; kind = "skill"; contents = request.text; }
    else if (request.text.startsWith("{")) {
      const parsed = JSON.parse(request.text) as { env?: Record<string, string>; headers?: Record<string, string> };
      secrets = Object.keys(parsed.env ?? parsed.headers ?? {});
    } else return { method: request.method, check: { items: [], error: "Invalid draft", needsName: false } };
    if (!name) return { method: request.method, check: { items: [], error: null, needsName: true } };
    return { method: request.method, check: { items: [{ name, kind, detail: kind === "skill" ? "SKILL.md" : "run-mcp", remote: name === "External", summary: "Draft", secrets, ...(contents !== undefined ? { contents } : {}) }], error: null, needsName: false } };
  }
  if (request.method === "addPlugin") { adds.push(request); return { method: request.method, job: "" }; }
  if (request.method === "importPlugin") { imports.push(request); return { method: request.method, job: "" }; }
  throw new Error(`Unexpected ${request.method}`);
});
useApp.setState({ selection: { type: "settings", page: "plugins" }, projects: { fixture: {
  id: "fixture", name: "Project", createdAtMs: 0, prefs: {} as ProjectPrefs,
  repos: [{ path: "/fixture/project", name: "repo" }], trust: [{ path: "/fixture/project", trusted: true, decidedAtMs: 0 }],
} } });
const disconnect = connectPlugins(livePlugins());
createRoot(document.getElementById("root")!).render(<TooltipProvider><PluginsPage /></TooltipProvider>);
const dialog = () => document.querySelector<HTMLElement>("[data-dialog]")!;
const field = (label: string) => [...document.querySelectorAll<HTMLInputElement | HTMLTextAreaElement>("input,textarea")].find((entry) => entry.getAttribute("aria-label") === label)!;
const text = () => dialog()?.textContent ?? "";
const button = (label: string) => {
  const buttons = [...(dialog() ?? document).querySelectorAll<HTMLButtonElement>("button")];
  return buttons.find((candidate) => candidate.textContent === label || candidate.getAttribute("aria-label") === label)
    ?? buttons.find((candidate) => candidate.textContent?.startsWith(label));
};
const click = (label: string) => { const target = button(label); if (!target) throw new Error(`Missing ${label}: ${text()}`); target.click(); };
const type = (label: string, value: string) => {
  const target = field(label);
  const proto = target instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
  Object.getOwnPropertyDescriptor(proto, "value")!.set!.call(target, value);
  target.dispatchEvent(new Event("input", { bubbles: true }));
};
async function wait(check: () => boolean) {
  const until = performance.now() + 5000;
  while (!check()) { if (performance.now() > until) throw new Error(`Timed out: ${document.body.textContent}`); await new Promise((resolve) => setTimeout(resolve, 20)); }
}
async function open() { click("Add"); await wait(() => !!field("What to add")); }
async function finish() { await wait(() => !!button("Done")); click("Done"); await wait(() => !document.querySelector("[data-dialog]")); }
async function run() {
  await wait(() => !![...document.querySelectorAll("button")].find((entry) => entry.textContent === "Add"));
  await open();
  const sharedEditor = !!dialog().querySelector('[data-slot="code-file-header"]') && !!dialog().querySelector('[data-slot="code-editor"]') && dialog().classList.contains("max-w-plugin-file");
  const startsEmpty = field("What to add").value === "" && button("Save")!.disabled;
  type("What to add", "invalid"); click("Save");
  await wait(() => text().includes("Invalid draft"));
  const errorKeptEditor = field("What to add").value === "invalid";
  type("What to add", '{"command":"run-mcp","env":{"API_KEY":""}}'); click("Save");
  await wait(() => !!field("Name")); type("Name", "authored-server"); click("Save");
  await wait(() => dialog().getAttribute("data-dialog") === "add-review");
  click("Advanced"); await wait(() => !!field("API_KEY value")); type("API_KEY value", "fixture-value");
  const projectRadio = [...dialog().querySelectorAll<HTMLButtonElement>('[role="radio"]')].find((entry) => entry.textContent?.includes("Only in one project"))!;
  projectRadio.click(); await wait(() => projectRadio.getAttribute("aria-checked") === "true"); click("Add"); await finish();

  await open(); click("Skill"); await wait(() => field("What to add").value.startsWith("---"));
  const starter = field("What to add").value;
  click("Choose folder…"); await wait(() => field("What to add").value === picked);
  const folderFilled = field("What to add").value;
  const edited = picked.replace("name: picked", "name: authored-skill").replace("Old instructions", "Edited instructions");
  type("What to add", edited);
  await wait(() => !!dialog().querySelector('[data-slot="code-editor"]')?.textContent?.includes("Edited instructions"));
  field("What to add").dispatchEvent(new KeyboardEvent("keydown", { code: "KeyS", key: "s", ctrlKey: true, bubbles: true, cancelable: true }));
  await wait(() => dialog().getAttribute("data-dialog") === "add-review"); click("Add"); await finish();

  await open(); click("Import from…"); await wait(() => !!button("Import External"));
  const importSources = text(); click("Import External");
  await wait(() => field("What to add").value.includes("Authorization"));
  const importPreview = field("What to add").value;
  type("What to add", importPreview.replace("https://old.test/mcp", "https://edited.test/mcp"));
  await wait(() => !!dialog().querySelector('[data-slot="code-editor"]')?.textContent?.includes("https://edited.test/mcp"));
  click("Save"); await wait(() => dialog().getAttribute("data-dialog") === "add-review"); click("Import"); await finish();
  disconnect();
  return { sharedEditor, startsEmpty, errorKeptEditor, starter, folderFilled, folders, importSources, importPreview,
    server: { projects: adds[0]?.projectIds, valueNames: Object.keys(adds[0]?.values ?? {}) },
    skill: { text: adds[1]?.text, source: adds[1]?.source }, imported: imports[0]?.contents };
}
void run().then((result) => { document.getElementById("result")!.textContent = JSON.stringify(result); }, (error: unknown) => {
  disconnect(); document.getElementById("result")!.textContent = JSON.stringify({ error: String(error) });
});
