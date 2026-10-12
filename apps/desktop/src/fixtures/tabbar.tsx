/**
 * Headless exercise of a session's top bar: a title with "+" at its end while nothing is open,
 * tabs with "+" after them once something is, their measures at both densities, the widths kept
 * after a close, and Delete on a tab. `compact=1` runs it at compact density.
 */
import { AssistantRuntimeProvider, type ThreadMessage, useExternalStoreRuntime } from "@assistant-ui/react";
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { SessionTabBar } from "@/app/conversation/SessionTabBar";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { Conversation, Request } from "@/ipc/generated";
import { closeTab, newSessionTab, selectTab, sessionTabs, useSessionTabs } from "@/state/sessionTabs";
import { useApp } from "@/state/store";

const params = new URLSearchParams(location.search);
if (params.has("compact")) document.documentElement.dataset.density = "compact";
// Headless virtual time doesn't run CSS transitions: the checks read where they end.
document.head.append(Object.assign(document.createElement("style"), { textContent: "*, ::after { transition: none !important; }" }));
const id = "tabbar-fixture";
useSessionTabs.setState({ sessions: {} });
const conversation: Conversation = {
  id, kind: "session", projectId: "fixture-project", title: "Tidy the release notes",
  pinnedAtMs: null, createdAtMs: 0, updatedAtMs: 0, setup: null, lifecycle: "active",
  forkedFrom: null, sideOf: null, fallback: null, quotaWait: null,
};
useApp.setState({
  conversations: { [id]: conversation },
  selection: { type: "conversation", id },
  info: { platform: "macos", version: "fixture", processStartMs: 0, smoke: false, firstLaunch: false, budgetTolerance: 1 },
  connection: { status: "connected", daemon: null, reason: null },
});
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  return { method: (payload as { request: Request }).request.method };
});

const bar = () => document.querySelector<HTMLElement>('[data-slot="session-tabs"]')!;
const box = (element: Element | null | undefined) => element?.getBoundingClientRect() ?? null;
const labels = () => [...bar().querySelectorAll('[role="tab"]')].map((tab) => tab.textContent?.trim());
const tabEl = (tabId: string) => bar().querySelector<HTMLElement>(`[data-tab-id="${CSS.escape(tabId)}"]`)!;
const contentTabs = () => [...bar().querySelectorAll<HTMLElement>("[data-session-tab]")];
const widths = () => contentTabs().map((tab) => Math.round(tab.getBoundingClientRect().width));
const separator = (tab: HTMLElement) => {
  const style = getComputedStyle(tab, "::after");
  return { height: parseFloat(style.height), opacity: style.opacity, color: style.backgroundColor };
};
const press = (target: Element, code: string, key: string) =>
  target.dispatchEvent(new KeyboardEvent("keydown", { code, key, bubbles: true, cancelable: true }));

function Fixture() {
  const runtime = useExternalStoreRuntime<ThreadMessage>({ messages: [], onNew: async () => {} });
  const [result, setResult] = useState("");
  useEffect(() => {
    const seen: Record<string, unknown> = {};
    const timers: ReturnType<typeof setTimeout>[] = [];
    const at = (ms: number, action: () => void) => timers.push(setTimeout(action, ms));
    const newTab = () => bar().querySelector<HTMLButtonElement>('button[aria-label="New tab"]')!;
    const context = () => document.querySelector<HTMLButtonElement>('button[aria-label="Context"]')!;
    at(500, () => {
      seen.plain = bar().hasAttribute("data-plain");
      seen.plainTabs = bar().querySelectorAll('[role="tab"]').length;
      seen.plainTitle = bar().textContent;
      seen.plainPlusNext = newTab().nextElementSibling?.getAttribute("aria-label");
      seen.plainPlusGap = Math.round(box(context())!.left - box(newTab())!.right);
      newTab().click();
    });
    at(900, () => {
      seen.opened = bar().hasAttribute("data-plain");
      seen.openedLabels = labels();
      const last = contentTabs().at(-1)!;
      seen.plusAfterLast = Math.round(box(newTab())!.left - box(last)!.right);
      seen.plusFarFromContext = Math.round(box(context())!.left - box(newTab())!.right);
      const tab = getComputedStyle(last);
      seen.measure = { height: parseFloat(tab.height), radius: parseFloat(tab.borderTopLeftRadius), font: parseFloat(tab.fontSize) };
      seen.activeShadow = tab.boxShadow !== "none";
      for (const kind of ["browser", "terminal", "sideChat"] as const) newSessionTab(id, kind, "/fixture");
    });
    at(1300, () => {
      const [first, second, , fourth] = sessionTabs(id).tabs;
      selectTab(id, fourth!.id);
      seen.firstId = first!.id;
      seen.secondId = second!.id;
    });
    at(1700, () => {
      const [first, , third, fourth] = sessionTabs(id).tabs;
      seen.betweenInactive = separator(tabEl(first!.id));
      seen.besideActive = separator(tabEl(third!.id));
      seen.activeNone = separator(tabEl(fourth!.id));
      seen.before = widths();
      seen.clamped = widths().every((width) => width >= 89 && width <= 241);
      // The × of the second tab, as a pointer would: the others keep their widths.
      tabEl(sessionTabs(id).tabs[1]!.id).querySelector<HTMLButtonElement>('button[aria-label^="Close"]')!.click();
    });
    at(2000, () => {
      seen.held = widths();
      bar().querySelector('[role="tablist"]')!.dispatchEvent(new PointerEvent("pointerout", { bubbles: true, relatedTarget: document.body }));
    });
    at(2700, () => {
      seen.released = widths();
      // Delete in a text field types; on a tab it closes that tab.
      const field = document.querySelector<HTMLInputElement>("#field")!;
      field.focus();
      seen.fieldBefore = sessionTabs(id).tabs.length;
      press(field, "Backspace", "Backspace");
      seen.fieldAfter = sessionTabs(id).tabs.length;
      const first = tabEl(sessionTabs(id).tabs[0]!.id).querySelector<HTMLElement>('[role="tab"]')!;
      first.focus();
      press(first, "Backspace", "Backspace");
      seen.deleted = sessionTabs(id).tabs.length;
    });
    at(2900, () => {
      seen.focusAfterDelete = document.activeElement?.closest("[data-tab-id]")?.getAttribute("data-tab-id") === sessionTabs(id).active;
      const chat = tabEl("chat").querySelector<HTMLElement>('[role="tab"]')!;
      chat.focus();
      press(chat, "Delete", "Delete");
      seen.chatKept = sessionTabs(id).tabs.length;
    });
    at(3100, () => {
      for (const tab of sessionTabs(id).tabs) closeTab(id, tab.id);
    });
    at(3400, () => {
      seen.back = bar().hasAttribute("data-plain");
      setResult(JSON.stringify(seen));
    });
    return () => timers.forEach(clearTimeout);
  }, []);
  return (
    <AssistantRuntimeProvider runtime={runtime}>
      <main className="bg-background text-foreground w-narrowest-window flex h-screen min-w-0 flex-col">
        <SessionTabBar conversation={conversation} onRename={() => {}}>
          <button type="button" aria-label="Context">Context</button>
        </SessionTabBar>
        <input id="field" aria-label="Field" defaultValue="text" />
        {result && <pre id="tabbar-result">{result}</pre>}
      </main>
    </AssistantRuntimeProvider>
  );
}
createRoot(document.getElementById("root")!).render(<TooltipProvider><Fixture /></TooltipProvider>);
