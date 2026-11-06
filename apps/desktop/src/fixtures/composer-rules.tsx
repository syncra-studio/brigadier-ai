/**
 * A standalone Vite entry, excluded from the app entry and production build: a new chat's
 * composer with the rules file capping effort at High (issue 545), IPC mocked. The draft was left on
 * Max: the picker shows High, and Extra high and Max greyed with the rule's words. Send is then
 * refused by the daemon (a rule added since): the banner says the rule in its words and the typed
 * message stays. What it saw goes as JSON into `<pre id="composer-rules-result">`.
 *
 * The composer's editor, which focuses itself as it loads, is held until the closed picker hands
 * the keyboard back to its trigger, and the fixture looks again only once the editor has taken it:
 * a slow machine's order (#682).
 */
import { mockIPC } from "@/ipc/nativeMocks";
import { createRoot } from "react-dom/client";

import { updateDraft } from "@/app/conversation/draftSetup";
import { ConversationView } from "@/app/conversation/ConversationView";
import { SidebarProvider } from "@/components/layout";
import { TooltipProvider } from "@/components/overlays";
import { model, overview } from "@/fixtures/agents";
import { IDLE_RESEARCH } from "@/fixtures/rankings";
import { fixtureClick, fixtureInput } from "@/fixtures/input";
import type { ModelRules, Request } from "@/ipc/generated";
import { revealApp } from "@/lib/splash";
import { select } from "@/state/actions";
import { useApp } from "@/state/store";

localStorage.clear();
// Startup isn't over, so the composer's editor waits to load (whenRevealed).
document.documentElement.dataset["splash"] = "";
const rules: ModelRules = { path: "/tmp/models.md", revision: "fixture", shipped: [], rows: [], rules: [{ type: "effortCeiling", effort: "high" }], suggestions: [], research: IDLE_RESEARCH };
const efforts = { efforts: ["low", "medium", "high", "xhigh", "max"], defaultEffort: "high" };
let created = 0;
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  switch (req.method) {
    case "getModelRules":
      return { method: req.method, rules };
    case "createConversation":
      created += 1;
      // As the daemon refuses: a rule the composer hadn't read yet.
      throw { code: "invalid", message: 'refused by your model rule: "effort-ceiling: high"' };
    default:
      return { method: req.method };
  }
});
const state = useApp.getState();
useApp.setState({
  connection: { status: "connected", daemon: null, reason: null },
  providers: { ...state.providers, view: { providers: [overview("claude", [model("opus", "Opus 5.5", { isDefault: true, ...efforts })])], sessions: [], fixtures: [] } },
});
// The draft was left on Max before the rule was written.
updateDraft(null, { model: { provider: "claude", model: "opus", effort: "max" } });

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
/** While set, the fixture doesn't look: something it mustn't see yet is under way. */
let held: Promise<unknown> | null = null;
async function until(what: string, check: () => boolean, ms = 10_000) {
  const end = performance.now() + ms;
  for (;;) {
    if (held) await held;
    if (check()) return;
    if (performance.now() > end) throw new Error(`timed out waiting for ${what}`);
    await sleep(16);
  }
}
const field = () => document.querySelector<HTMLElement>("[data-slot=composer-input] [contenteditable=true]");
const typed = () => field()?.textContent ?? "";
const trigger = () => document.querySelector<HTMLElement>("[data-slot=model-selector-trigger]");
const menu = () => document.querySelector<HTMLElement>("[data-slot=model-picker-popover]");
const banner = () => document.querySelector<HTMLElement>("[data-slot=banner]");

async function drive() {
  const seen: Record<string, unknown> = {};
  try {
    await until("the rules read and the picker on High", () => trigger()?.textContent?.includes("High") ?? false);
    seen.trigger = trigger()!.textContent;
    trigger()!.focus();
    trigger()!.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
    await until("the picker", () => menu() !== null);
    seen.efforts = [...menu()!.querySelectorAll<HTMLElement>("[data-slot=model-picker-effort] [role=radio]")].map((item) => ({
      text: item.textContent,
      disabled: item.getAttribute("aria-disabled") === "true",
      checked: item.dataset.state === "on",
    }));
    // The closed picker hands the keyboard back to its trigger first: the field takes it after.
    // Heard as it happens: the editor taking the keyboard as it loads may end it before a look.
    let back = false;
    trigger()!.addEventListener("focus", () => (back = true), { once: true });
    // The moment the closed picker hands the keyboard back to its trigger, the editor loads, and
    // takes the keyboard as it does on loading, before the fixture looks again.
    trigger()!.addEventListener(
      "focus",
      () => {
        const taken = new Promise<void>((resolve) => {
          const onFocus = () => {
            if (document.activeElement !== field()) return;
            document.removeEventListener("focusin", onFocus);
            resolve();
          };
          document.addEventListener("focusin", onFocus);
        });
        held = Promise.race([taken, sleep(10_000)]).then(() => {
          held = null;
        });
        void revealApp();
      },
      { once: true },
    );
    menu()!.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    await until("the picker closed", () => menu() === null);
    await until("the keyboard back on the picker", () => back);
    await until("the field", () => field() !== null);
    await fixtureClick(field()!);
    await until("the field focused", () => document.activeElement === field());
    await fixtureInput({ type: "type", text: "Reply with the single word OK." });
    await until("the text typed", () => typed() === "Reply with the single word OK.");
    document.querySelector<HTMLButtonElement>(".aui-composer-send")!.click();
    await until("the refusal", () => banner() !== null);
    await sleep(600);
    seen.created = created;
    seen.banner = banner()!.querySelector("[data-slot=banner-text]")?.textContent;
    seen.kept = typed();
  } catch (error) {
    seen.error = String(error);
  }
  const out = document.createElement("pre");
  out.id = "composer-rules-result";
  out.textContent = JSON.stringify(seen);
  document.body.append(out);
}

select({ type: "draft", kind: "chat" });
createRoot(document.getElementById("root")!).render(
  <TooltipProvider>
    <SidebarProvider className="bg-background flex h-screen flex-col" defaultOpen={false}>
      <main className="relative flex min-h-0 w-full flex-1 flex-col">
        <ConversationView key="draft" selection={{ type: "draft", kind: "chat" }} />
      </main>
    </SidebarProvider>
  </TooltipProvider>,
);
void drive();
