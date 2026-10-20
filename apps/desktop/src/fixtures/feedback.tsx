/**
 * Send feedback over a faked app: the sidebar's foot and the dialog. ?drive=1 opens it from the
 * foot's Help menu, writes a bug, looks at the idea form, sends it (which closes it), opens a draft and writes
 * what it saw (and the link opened) to #feedback-result (FeedbackRender.test.ts reads it).
 * ?look=1 sets the open dialog's parts beside the app's shared ones and writes their looks to
 * #feedback-look. Without either, the dialog stays open, filled in, for screenshots.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { FeedbackDialog } from "@/app/shell/feedback/FeedbackDialog";
import { SidebarFoot } from "@/app/shell/SidebarFoot";
import { SidebarProvider } from "@/components/layout";
import { TooltipProvider } from "@/components/overlays";
import { Canon, canon, look, type LookPairs } from "@/fixtures/dialogLook";
import type { ProviderOverview } from "@/ipc/generated";
import { openFeedback, useFeedbackUi } from "@/state/feedback";
import { useApp } from "@/state/store";

const params = new URLSearchParams(location.search);
// Driven, nothing animates (see the archived chats fixture).
if (params.has("drive") || params.has("look")) {
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
  const method = (payload as { request?: { method: string } }).request?.method;
  // No writer runs here: Send opens the one form.
  if (method === "writeUpFeedback") throw { code: "invalid", message: "no model can write the report now" };
  if (command === "ipc_request") return { method };
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
    view: {
      providers: [provider("claude", "2.1.4"), provider("codex", params.has("drive") ? null : "0.163.0")],
      sessions: [],
      fixtures: [],
    },
  },
}));
// Undriven, it opens with the design's note, for a screenshot beside it.
if (!params.has("drive")) {
  openFeedback({
    kind: "bug",
    text: "The usage popover doesn't update after a session finishes. I expected the 5-hour bar to move right away, but it only changed after I reopened it.",
  });
}

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
  const [looked, setLooked] = useState("");
  useEffect(() => {
    if (!params.has("look")) return;
    void (async () => {
      for (let tries = 0; tries < 100 && !dialog()?.querySelector("li"); tries++) await wait(50);
      const open = dialog();
      // The note as it rests, as the shared field is measured.
      open?.querySelector("textarea")?.blur();
      await wait(100);
      const pairs: LookPairs = {
        card: [look(open, "surface"), canon("dialog", "surface")],
        title: [look(open?.querySelector("h2"), "text"), canon("title", "text")],
        line: [look(open?.querySelector("p"), "text"), canon("line", "text")],
        close: [look(button("Close", open ?? document), "button"), canon("icon", "button")],
        kinds: [look(open?.querySelector('[role="radio"]')?.parentElement, "fill"), canon("segmented", "fill", "> *")],
        kind: [look(open?.querySelector('[role="radio"][data-state="on"]'), "button"), canon("segmented", "button", '[data-state="on"]')],
        note: [look(open?.querySelector("textarea"), "surface"), canon("input", "surface")],
        chip: [look(open?.querySelector('[aria-label="Also included"] li'), "button"), canon("badge", "button")],
        footer: [look(open?.lastElementChild, "edgeTop"), canon("divider", "edgeTop")],
        cancel: [look(button("Cancel", open ?? document), "button"), canon("ghost", "button")],
        send: [look(button("Continue on GitHub", open ?? document), "button"), canon("primary", "button")],
      };
      setLooked(JSON.stringify(pairs));
    })();
  }, []);
  useEffect(() => {
    if (!params.has("drive")) return;
    let stopped = false;
    void (async () => {
      const seen: Record<string, unknown> = {};
      for (let tries = 0; tries < 100 && !button("Help"); tries++) await wait(50);
      await wait(200);
      button("Help")?.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, button: 0 }));
      await wait(200);
      [...document.querySelectorAll<HTMLElement>('[role="menuitem"]')].find((item) => text(item) === "Send feedback…")?.click();
      await wait(300);
      const open = dialog();
      const area = open?.querySelector("textarea");
      seen.opened = open !== null;
      seen.title = text(open?.querySelector("h2"));
      seen.description = text(open?.querySelector("p"));
      seen.footer = text(open?.lastElementChild?.querySelector("span"));
      seen.kinds = [...(open?.querySelectorAll('[role="radio"]') ?? [])].map((item) => [text(item), item.getAttribute("data-state")]);
      seen.placeholder = area?.placeholder;
      seen.focused = document.activeElement === area;
      seen.included = [...(open?.querySelectorAll('[aria-label="Also included"] li') ?? [])].map(text);
      seen.sendDisabled = button("Continue on GitHub", open ?? document)?.disabled;
      seen.width = Math.round(open?.getBoundingClientRect().width ?? 0);
      button("An idea", open ?? document)?.click();
      await wait(100);
      seen.ideaPlaceholder = dialog()?.querySelector("textarea")?.placeholder;
      button("Something’s wrong", dialog() ?? document)?.click();
      await wait(100);
      const typed = dialog()?.querySelector("textarea");
      if (typed) type(typed, "The tab keeps spinning\nafter I stop the session.");
      await wait(100);
      seen.sendEnabled = button("Continue on GitHub", dialog() ?? document)?.disabled === false;
      button("Continue on GitHub", dialog() ?? document)?.click();
      await wait(300);
      seen.opened_urls = opened;
      seen.stillOpen = useFeedbackUi.getState().open;
      // A draft from elsewhere opens filled in, its kind chosen.
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
      {params.has("look") && <Canon />}
      {looked && <pre id="feedback-look">{looked}</pre>}
    </TooltipProvider>
  );
}

const root = document.getElementById("root");
if (root) createRoot(root).render(<Fixture />);
