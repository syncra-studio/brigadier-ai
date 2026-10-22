/**
 * Settings → Configuration beside a session's composer permission picker, over the IPC mock.
 * No daemon.
 *
 * ?level=ask (the default) | acceptEdits | full: the session's level.
 * ?drive=1 picks a new default on the page, switches the session from its picker (through the
 * Full access confirmation), and writes what it saw to #access-result
 * (ConfigurationPageRender.test.ts reads it).
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { ConversationPermissionPicker } from "@/app/shell/SetupPickers";
import { ViewContext } from "@/app/conversation/viewContext";
import { ConfigurationPage } from "@/app/settings/ConfigurationPage";
import { TooltipProvider } from "@/components/overlays";
import type { Conversation, PermissionLevel, Request } from "@/ipc/generated";
import { resolvePermission } from "@/lib/setup";
import { useApp } from "@/state/store";

const params = new URLSearchParams(location.search);
const id = "access-fixture";
const conversation = {
  id,
  kind: "session",
  projectId: null,
  title: "Install Homebrew",
  pinnedAtMs: null,
  createdAtMs: 0,
  updatedAtMs: 0,
  lifecycle: "active",
  forkedFrom: null,
  sideOf: null,
  fallback: null,
  quotaWait: null,
  setup: {
    type: "session",
    repo: "/tmp/access-fixture",
    environment: { type: "localCheckout", branch: "main" },
    permission: (params.get("level") ?? "ask") as PermissionLevel,
    planMode: false,
    workersSeeUncommitted: null,
    orchestrator: { provider: "claude", model: null, effort: null },
  },
} as Conversation;
useApp.setState((state) => ({
  settings: { ...state.settings, defaultPermission: "ask" },
  conversations: { [id]: conversation },
}));
const calls: string[] = [];

mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  switch (req.method) {
    case "updateSettings":
      calls.push(`default ${req.settings.defaultPermission}`);
      return { method: req.method, settings: req.settings };
    case "updateSetup":
      calls.push(`session ${req.setup.type === "session" ? req.setup.permission : "chat"}`);
      return { method: req.method, conversation: { ...useApp.getState().conversations[id], setup: req.setup } };
    case "listMemories":
      return { method: req.method, memories: [] };
    default:
      return { method: req.method };
  }
});

const q = <T extends Element = HTMLElement>(selector: string) => document.querySelector<T>(selector);
const buttonNamed = (label: string) =>
  [...document.querySelectorAll<HTMLButtonElement>("button")].find((button) => button.textContent?.trim() === label);
const picker = () => q('[data-slot="permission-picker"]')?.textContent?.trim();
const selectTrigger = () => q<HTMLButtonElement>('button[aria-label="Default permission level"]');
const sessionLevel = () => {
  const setup = useApp.getState().conversations[id]?.setup;
  return setup?.type === "session" ? setup.permission : null;
};

function Fixture() {
  const live = useApp((s) => s.conversations[id] ?? null);
  const [result, setResult] = useState("");
  useEffect(() => {
    if (!params.has("drive")) return;
    const seen: Record<string, unknown> = {};
    let stopped = false;
    // Each step waits for what it acts on (a menu, the confirmation) instead of a fixed time,
    // so a slow render doesn't skip one; after 5 s it goes on and the result shows what's missing.
    const until = async <T,>(find: () => T | null | undefined | false): Promise<T | undefined> => {
      for (let waited = 0; waited < 5000 && !stopped; waited += 50) {
        const found = find();
        if (found) return found;
        await new Promise((resolve) => setTimeout(resolve, 50));
      }
      return undefined;
    };
    // An item of the menu `trigger` opened, never of another one still closing.
    const item = (trigger: Element | null, label: string) => {
      const menu = trigger?.getAttribute("aria-expanded") === "true" && document.getElementById(trigger.getAttribute("aria-controls") ?? "");
      return menu ? [...menu.querySelectorAll<HTMLElement>('[role="menuitemradio"]')].find((item) => item.textContent?.startsWith(label)) : undefined;
    };
    const pill = () => q('[data-slot="permission-picker"]');
    void (async () => {
      await until(() => selectTrigger() && picker());
      seen.first = { select: selectTrigger()?.textContent, picker: picker() };
      selectTrigger()?.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, button: 0 }));
      (await until(() => item(selectTrigger(), "Accept edits")))?.click();
      await until(() => selectTrigger()?.textContent === "Accept edits" && selectTrigger()?.getAttribute("aria-expanded") === "false");
      const settings = useApp.getState().settings;
      seen.saved = {
        select: selectTrigger()?.textContent,
        newSession: resolvePermission(null, settings),
        pickedForOne: resolvePermission("full", settings),
        session: sessionLevel(),
        picker: picker(),
      };
      // The pill opens its menu on ↑, as the composer's keyboard path does.
      pill()?.focus();
      pill()?.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowUp", bubbles: true, cancelable: true }));
      (await until(() => item(pill(), "Full access")))?.click();
      const confirm = await until(() => buttonNamed("Confirm"));
      seen.confirming = !!confirm;
      confirm?.click();
      await until(() => sessionLevel() === "full" && picker() === "Full access");
      if (stopped) return;
      seen.switched = { session: sessionLevel(), picker: picker() };
      seen.calls = calls;
      setResult(JSON.stringify(seen));
    })();
    return () => {
      stopped = true;
    };
  }, []);
  return (
    <TooltipProvider>
      <main className="bg-background text-foreground grid h-screen grid-cols-2 overflow-hidden">
        <div className="border-divider min-h-0 border-e">
          <ConfigurationPage />
        </div>
        <ViewContext.Provider value={{ selection: { type: "conversation", id }, conversation: live, embedded: false }}>
          <div className="mx-auto flex w-full max-w-thread flex-col gap-3 overflow-y-auto px-5 py-10 text-sm">
            <p className="bg-foreground/5 self-end rounded-2xl px-3 py-2">Install Homebrew and set up the project.</p>
            {live && (
              <div className="@container/composer mt-6 flex items-center gap-2">
                <ConversationPermissionPicker conversation={live} />
              </div>
            )}
          </div>
        </ViewContext.Provider>
      </main>
      {result && <pre id="access-result">{result}</pre>}
    </TooltipProvider>
  );
}
createRoot(document.getElementById("root")!).render(<Fixture />);
