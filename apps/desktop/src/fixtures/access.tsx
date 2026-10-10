/**
 * Settings → Configuration beside a session's thread notice that the sandbox stopped what the
 * user asked for, and the session's composer permission picker, over the IPC mock. No daemon.
 *
 * ?level=askForApproval (the default) | approveForMe | fullAccess: the session's level.
 * ?drive=1 picks a new default on the page, switches the session from the notice (through the
 * Full access confirmation), opens Settings > Configuration from it, and writes what it saw to
 * #access-result (AccessRender.test.ts reads it).
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { ThreadStep } from "@/app/conversation/activity/Notice";
import { ConversationPermissionPicker } from "@/app/conversation/SetupPickers";
import { ViewContext } from "@/app/conversation/viewContext";
import { ConfigurationPage } from "@/app/settings/ConfigurationPage";
import { TooltipProvider } from "@/components/ui/tooltip";
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
    permission: (params.get("level") ?? "askForApproval") as PermissionLevel,
    planMode: false,
    workersSeeUncommitted: null,
    orchestrator: { provider: "claude", model: null, effort: null },
  },
} as Conversation;
useApp.setState((state) => ({
  settings: { ...state.settings, defaultPermission: "askForApproval" },
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
    default:
      return { method: req.method };
  }
});

const q = <T extends Element = HTMLElement>(selector: string) => document.querySelector<T>(selector);
const buttonNamed = (label: string) =>
  [...document.querySelectorAll<HTMLButtonElement>("button")].find((button) => button.textContent?.trim() === label);
const notice = () => q('[data-kind="fullAccessSuggested"]');
const noticeButtons = () => [...(notice()?.querySelectorAll("button") ?? [])].map((button) => button.textContent?.trim());
const defaultRow = () =>
  [...document.querySelectorAll('[data-setting]')].find((row) => row.textContent?.endsWith("Default"))?.getAttribute("data-setting");
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
    const at = (ms: number, run: () => void) => setTimeout(run, ms);
    const timers = [
      at(300, () => {
        seen.first = { select: selectTrigger()?.textContent, defaultRow: defaultRow(), notice: noticeButtons(), picker: picker() };
        selectTrigger()?.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, button: 0 }));
      }),
      at(600, () => {
        [...document.querySelectorAll<HTMLElement>('[role="menuitemradio"]')]
          .find((item) => item.textContent?.startsWith("Approve for me"))
          ?.click();
      }),
      at(900, () => {
        const settings = useApp.getState().settings;
        seen.saved = {
          select: selectTrigger()?.textContent,
          defaultRow: defaultRow(),
          newSession: resolvePermission(null, settings),
          pickedForOne: resolvePermission("fullAccess", settings),
          session: sessionLevel(),
          picker: picker(),
        };
        buttonNamed("Switch this session to Full access")?.click();
      }),
      at(1200, () => {
        seen.confirming = !!buttonNamed("Confirm");
        buttonNamed("Confirm")?.click();
      }),
      at(1500, () => {
        seen.switched = {
          session: sessionLevel(),
          picker: picker(),
          notice: noticeButtons(),
          line: q('[data-slot="full-access-on"]')?.textContent,
        };
        buttonNamed("Open Settings > Configuration")?.click();
      }),
      at(1800, () => {
        seen.selection = useApp.getState().selection;
        seen.calls = calls;
        setResult(JSON.stringify(seen));
      }),
    ];
    return () => timers.forEach(clearTimeout);
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
            <ThreadStep
              step={{ kind: { type: "fullAccessSuggested", reason: "Installing Homebrew writes to /opt/homebrew, outside the project, and needs the internet." }, position: 1 }}
            />
            <p>It needs Full access: the sandbox only lets this session write in the project.</p>
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
