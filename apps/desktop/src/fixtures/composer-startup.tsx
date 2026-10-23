/**
 * A standalone Vite entry, excluded from the app entry and production build: a new chat's
 * composer as the app starts (#469), IPC mocked. It mounts under the startup screen, with the
 * plain field standing in for the editor, and is measured there; the startup screen goes, the
 * editor takes the field's place and it is measured again; then another new chat's composer
 * mounts. What it saw goes as JSON into `<pre id="composer-startup-result">`.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { ConversationView } from "@/app/conversation/ConversationView";
import { SidebarProvider } from "@/components/layout";
import { TooltipProvider } from "@/components/overlays";
import { revealApp } from "@/lib/splash";
import { select } from "@/state/actions";
import { type Selection, useApp } from "@/state/store";

// As index.html has it until the app is shown.
document.documentElement.dataset.splash = "";
localStorage.clear();
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  return { method: (payload as { request: { method: string } }).request.method };
});
useApp.setState({ connection: { status: "connected", daemon: null, reason: null } });

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
async function until(what: string, check: () => boolean, ms = 10_000) {
  const end = performance.now() + ms;
  while (!check()) {
    if (performance.now() > end) throw new Error(`timed out waiting for ${what}`);
    await sleep(20);
  }
}

const composer = () => document.querySelector<HTMLElement>("[data-slot=composer]");
const railItem = () => composer()?.querySelector<HTMLElement>("[data-slot=composer-rail-item]") ?? null;
/** The composer's height, its field's kind and whether its rail item opens. */
function measure() {
  const item = railItem();
  return {
    height: composer()?.offsetHeight ?? 0,
    field: composer()?.querySelector("textarea") ? "stand-in" : composer()?.querySelector("[contenteditable]") ? "editor" : "none",
    railOpens: item ? getComputedStyle(item).animationName === "rail-open" : null,
  };
}

const root = createRoot(document.getElementById("root")!);
function show(selection: Selection, key: string) {
  select(selection);
  root.render(
    <TooltipProvider>
      <SidebarProvider className="bg-background flex h-screen flex-col" defaultOpen={false}>
        <main className="relative flex min-h-0 w-full flex-1 flex-col">
          <ConversationView key={key} selection={selection} />
        </main>
      </SidebarProvider>
    </TooltipProvider>,
  );
}

async function run() {
  const seen: Record<string, unknown> = {};
  show({ type: "draft", kind: "chat" }, "first");
  await until("the composer and its rail", () => railItem() !== null && measure().field === "stand-in");
  await sleep(400);
  seen.underStartup = measure();
  await revealApp();
  await until("the editor", () => measure().field === "editor");
  await sleep(100);
  seen.revealed = measure();
  // A new chat has no reply to jump to, so no "Latest response" heading (#564).
  seen.latestResponse = document.querySelectorAll("[data-slot=latest-response]").length;
  const before = railItem();
  show({ type: "draft", kind: "chat" }, "second");
  await until("the next composer", () => railItem() !== null && railItem() !== before);
  seen.later = measure();
  return seen;
}

void run()
  .catch((error: unknown) => ({ error: String(error) }))
  .then((seen) => {
    const out = document.createElement("pre");
    out.id = "composer-startup-result";
    out.textContent = JSON.stringify(seen);
    document.body.append(out);
  });
