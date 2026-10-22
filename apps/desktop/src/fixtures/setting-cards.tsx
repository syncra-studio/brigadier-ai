/**
 * The thread's settings cards (`suggest_setting`) under a request, over the IPC mock: a
 * permission level and the built-in browser. Pressing a card's button makes
 * its change, and the card then says so. No daemon.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { ThreadStep } from "@/app/conversation/activity/Notice";
import { ViewContext } from "@/app/conversation/viewContext";
import { TooltipProvider } from "@/components/overlays";
import type { Conversation, Request } from "@/ipc/generated";
import { useApp } from "@/state/store";

const id = "setting-cards-fixture";
const conversation = {
  id, kind: "session", projectId: null, title: "Ship the todo app", pinnedAtMs: null, createdAtMs: 0, updatedAtMs: 0,
  lifecycle: "active", forkedFrom: null, sideOf: null, fallback: null, quotaWait: null,
  setup: {
    type: "session", repo: "/tmp/setting-cards-fixture", environment: { type: "localCheckout", branch: "main" },
    permission: "full", planMode: false, workersSeeUncommitted: null,
    orchestrator: { provider: "claude", model: null, effort: null },
  },
} as Conversation;
useApp.setState((state) => ({
  settings: { ...state.settings, browserEnabled: false },
  conversations: { [id]: conversation },
}));

mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  if (req.method === "updateSettings") return { method: req.method, settings: req.settings };
  return { method: req.method };
});

function Fixture() {
  return (
    <TooltipProvider>
      <ViewContext.Provider value={{ selection: { type: "conversation", id }, conversation, embedded: false }}>
        <main className="bg-background text-foreground mx-auto flex h-screen w-full max-w-thread flex-col gap-3 overflow-y-auto px-5 py-10 text-sm">
          <p className="bg-foreground/5 self-end rounded-2xl px-3 py-2">Check the page in the browser, and ask me before each step.</p>
          <ThreadStep step={{ kind: { type: "settingSuggested", change: { setting: "browser", on: true }, reason: "Checking the page needs the built-in browser, which is off." }, position: 2 }} />
          <ThreadStep step={{ kind: { type: "settingSuggested", change: { setting: "permission", level: "ask" }, reason: "You want to approve each step before it runs." }, position: 3 }} />
          <p>Each waits on your press; nothing changes until then.</p>
        </main>
      </ViewContext.Provider>
    </TooltipProvider>
  );
}
createRoot(document.getElementById("root")!).render(<Fixture />);
