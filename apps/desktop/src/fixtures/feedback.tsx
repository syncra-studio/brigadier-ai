/**
 * Send feedback over a faked app: the sidebar's foot and the dialog. ?drive=1 opens it from the
 * foot's icon, writes a bug, looks at the idea form, sends the bug, opens a draft and writes
 * what it saw (and the link opened) to #feedback-result (FeedbackRender.test.ts reads it).
 * Without it, the dialog stays open for screenshots.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { FeedbackDialog } from "@/app/feedback/FeedbackDialog";
import { SidebarFoot } from "@/app/SidebarFoot";
import { SidebarProvider } from "@/components/ui/sidebar";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { ProviderOverview } from "@/ipc/generated";
import { closeFeedback, openFeedback, useFeedbackUi } from "@/state/feedback";
import { useApp } from "@/state/store";

const params = new URLSearchParams(location.search);
// Driven, nothing animates (see the archived chats fixture).
if (params.has("drive")) {
  const still = document.createElement("style");
  still.textContent = "*, *::before, *::after { animation: none !important; transition: none !important; }";
  document.head.append(still);
}

const opened: string[] = [];
mockIPC((command, payload) => {
  if (command === "system_name") return "macOS 27.0 Tahoe (aarch64)";
  if (command === "open_url") {
    opened.push((payload as { url: string }).url);
    return null;
  }
  if (command === "ipc_request") return { method: (payload as { request: { method: string } }).request.method };
  return null;
});

const provider = (kind: "claude" | "codex", version: string | null): ProviderOverview => ({
  provider: kind,
  status:
    version === null
      ? null
      : { provider: kind, path: null, version, loggedIn: true, authMethod: null, plan: null } as ProviderOverview["status"],
  models: null,
  quota: null,
  usage: null,
  error: null,
  checkedAtMs: null,
});
useApp.setState((state) => ({
  info: { ...state.info, version: "0.1.0", platform: "macos" } as typeof state.info,
  providers: {
    ...state.providers,
    view: { providers: [provider("claude", "2.1.4"), provider("codex", null)], sessions: [], fixtures: [] },
  },
}));
if (!params.has("drive")) openFeedback();

const text = (element: Element | null | undefined) => element?.textContent?.trim() ?? "";
const dialog = () => document.querySelector<HTMLElement>('[data-slot="feedback"]');
const button = (name: string, root: ParentNode = document) =>
  [...root.querySelectorAll<HTMLButtonElement>("button")].find(
    (element) => text(element) === name || element.getAttribute("aria-label") === name,
  );
const type = (area: HTMLTextAreaElement, value: string) => {
  Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")?.set?.call(area, value);
  area.dispatchEvent(new Event("input", { bubbles: true }));
};
const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

function Fixture() {
  const [result, setResult] = useState("");
  useEffect(() => {
    if (!params.has("drive")) return;
    let stopped = false;
    void (async () => {
      const seen: Record<string, unknown> = {};
      await wait(200);
      button("Send feedback")?.click();
      await wait(300);
      const open = dialog();
      const area = open?.querySelector("textarea");
      seen.opened = open !== null;
      seen.title = text(open?.querySelector("h2"));
      seen.description = text(open?.querySelector("p"));
      seen.kinds = [...(open?.querySelectorAll('[role="radio"]') ?? [])].map((item) => [text(item), item.getAttribute("data-state")]);
      seen.placeholder = area?.placeholder;
      seen.focused = document.activeElement === area;
      seen.details = text(open?.querySelector("p.text-xs"));
      seen.sendDisabled = button("Open on GitHub", open ?? document)?.disabled;
      const style = open ? getComputedStyle(open) : null;
      seen.card = { width: Math.round(open?.getBoundingClientRect().width ?? 0), radius: Number.parseFloat(style?.borderTopLeftRadius ?? "0") };
      button("An idea", open ?? document)?.click();
      await wait(100);
      seen.ideaPlaceholder = dialog()?.querySelector("textarea")?.placeholder;
      button("Something’s wrong", dialog() ?? document)?.click();
      await wait(100);
      const typed = dialog()?.querySelector("textarea");
      if (typed) type(typed, "The tab keeps spinning\nafter I stop the session.");
      await wait(100);
      seen.sendEnabled = button("Open on GitHub", dialog() ?? document)?.disabled === false;
      button("Open on GitHub", dialog() ?? document)?.click();
      await wait(300);
      seen.opened_urls = opened;
      seen.sent = text(dialog()?.querySelector('[role="status"]'));
      seen.stillOpen = useFeedbackUi.getState().open;
      // A draft from elsewhere opens filled in, its kind chosen.
      closeFeedback();
      await wait(200);
      openFeedback({ kind: "idea", text: "Let me pin a chat" });
      await wait(300);
      seen.draft = {
        kind: text(dialog()?.querySelector('[role="radio"][aria-checked="true"]')),
        text: dialog()?.querySelector("textarea")?.value,
      };
      if (!stopped) setResult(JSON.stringify(seen));
    })();
    return () => {
      stopped = true;
    };
  }, []);
  return (
    <TooltipProvider>
      <SidebarProvider>
        <div className="bg-background text-foreground flex h-screen w-full items-end">
          <aside className="w-64 shrink-0">
            <SidebarFoot />
          </aside>
        </div>
        <FeedbackDialog />
      </SidebarProvider>
      {result && <pre id="feedback-result">{result}</pre>}
    </TooltipProvider>
  );
}

const root = document.getElementById("root");
if (root) createRoot(root).render(<Fixture />);
