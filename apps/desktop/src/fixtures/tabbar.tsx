/**
 * Headless exercise of a session's top bar: a title with ⋯, the toolbar and "+" last while nothing is open,
 * tabs with "+" after them once something is, their measures at both densities, the widths kept
 * after a close, and Delete on a tab. `compact=1` runs it at compact density.
 */
import { AssistantRuntimeProvider, type ThreadMessage, useExternalStoreRuntime } from "@assistant-ui/react";
import { mockIPC } from "@tauri-apps/api/mocks";
import { emit } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { SessionTabBar } from "@/app/conversation/SessionTabBar";
import { usePaneShortcuts } from "@/app/paneShortcuts";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { Conversation, Request } from "@/ipc/generated";
import { closeTab, newSessionTab, selectTab, sessionTabs, useSessionTabs } from "@/state/sessionTabs";
import { useApp } from "@/state/store";

const params = new URLSearchParams(location.search);
if (params.has("compact")) document.documentElement.dataset.density = "compact";
// Headless virtual time doesn't run CSS transitions: the checks read where they end.
if (!params.has("live")) document.head.append(Object.assign(document.createElement("style"), { textContent: "*, ::after { transition: none !important; }" }));
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
const eventListeners = new Map<number, string>();
mockIPC((command, payload) => {
  const values = payload as Record<string, unknown>;
  if (command === "plugin:event|listen") {
    eventListeners.set(values.handler as number, values.event as string);
    return values.handler;
  }
  if (command === "plugin:event|unlisten") {
    eventListeners.delete(values.eventId as number);
    return null;
  }
  if (command === "plugin:event|emit") {
    const bridge = (window as unknown as { __TAURI_INTERNALS__: { runCallback: (id: number, event: unknown) => void } })["__TAURI_INTERNALS__"];
    for (const [handler, event] of eventListeners) {
      if (event === values.event) bridge.runCallback(handler, { id: handler, event, payload: values.payload });
    }
    return null;
  }
  if (command !== "ipc_request") return null;
  return { method: (payload as { request: Request }).request.method };
});

const bar = () => document.querySelector<HTMLElement>('[data-slot="session-tabs"]')!;
const box = (element: Element | null | undefined) => element?.getBoundingClientRect() ?? null;
// The bar's controls in order, "|" for its divider.
const controls = () => [...bar().querySelectorAll<HTMLElement>("button, [data-bar-divider]")]
  .filter((element) => !element.closest('[role="tablist"]'))
  .map((element) => element.dataset.barDivider !== undefined ? "|" : element.getAttribute("aria-label") ?? element.textContent?.trim());
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
  usePaneShortcuts();
  const runtime = useExternalStoreRuntime<ThreadMessage>({ messages: [], onNew: async () => {} });
  const [result, setResult] = useState("");
  useEffect(() => {
    if (params.has("manual")) return;
    if (params.has("live")) {
      let disposed = false;
      void checkMotionAndMenu().then((seen) => { if (!disposed) setResult(JSON.stringify(seen)); });
      return () => { disposed = true; };
    }
    const seen: Record<string, unknown> = {};
    const timers: ReturnType<typeof setTimeout>[] = [];
    const at = (ms: number, action: () => void) => timers.push(setTimeout(() => {
      // Animations are finished first: these geometry checks inspect the settled strip.
      for (const animation of document.getAnimations()) animation.finish();
      action();
    }, ms));
    const newTab = () => bar().querySelector<HTMLButtonElement>('button[aria-label="New tab"]')!;
    const context = () => document.querySelector<HTMLButtonElement>('button[aria-label="Context"]')!;
    at(500, () => {
      // Alone, the chat is still a tab: its ⋯ inside it, "+" right after it.
      seen.plain = bar().hasAttribute("data-plain");
      seen.aloneLabels = labels();
      seen.aloneChatActions = [...tabEl("chat").querySelectorAll("button")].some((button) => button.textContent === "Chat actions");
      seen.aloneControls = controls();
      seen.alonePlusAfterChat = Math.round(box(newTab())!.left - box(tabEl("chat"))!.right);
      newTab().click();
    });
    at(900, () => {
      seen.openedLabels = labels();
      const last = contentTabs().at(-1)!;
      seen.plusAfterLast = Math.round(box(newTab())!.left - box(last)!.right);
      seen.plusFarFromContext = Math.round(box(context())!.left - box(newTab())!.right);
      seen.openedControls = controls();
      // Room is left after "+": no rule before the toolbar.
      seen.dividerWithRoom = bar().querySelector("[data-bar-divider]")!.hasAttribute("data-hidden");
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
      // The tabs fill the bar: the rule shows.
      seen.dividerWhenFull = !bar().querySelector("[data-bar-divider]")!.hasAttribute("data-hidden");
      // Chat is in back here: its ⋯ still shows.
      seen.chatActions = getComputedStyle([...bar().querySelectorAll('[role="tablist"] button')].find((button) => button.textContent === "Chat actions")!).opacity;
      seen.clamped = widths().every((width) => width >= 89 && width <= 241);
      // The scroller fills the list beside Chat, whatever its tabs' own widths would add up to.
      const scroller = tabEl(first!.id).parentElement!.parentElement!;
      seen.scroller = {
        basis: getComputedStyle(scroller).flexBasis,
        toListEnd: Math.round(box(bar().querySelector('[role="tablist"]'))!.right - box(scroller)!.right),
      };
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
      seen.back = { plain: bar().hasAttribute("data-plain"), labels: labels() };
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
        <div data-pane="browser" data-active="true" />
        {result && <pre id="tabbar-result">{result}</pre>}
      </main>
    </AssistantRuntimeProvider>
  );
}

/** Exercise the event bridge with the shell unfocused, and tab motion with real frames. */
const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
async function checkMotionAndMenu() {
  const seen: Record<string, unknown> = {};
  const ready = async (condition: () => boolean) => {
    for (let tries = 0; !condition() && tries < 500; tries++) await wait(10);
    if (!condition()) throw new Error("Tab fixture step did not become ready");
  };
  await ready(() => eventListeners.size > 0);
  const menu = async (name: string) => { await emit("pane-shortcut", name); await wait(20); };
  const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  seen.reduced = reduced;
  // A native page owns focus: the shell's browser host receives menu events.
  Object.defineProperty(document, "hasFocus", { configurable: true, value: () => false });
  const forwarded: string[] = [];
  const record = (event: KeyboardEvent) => {
    if (event.target instanceof Element && event.target.matches('[data-pane="browser"]')) forwarded.push(event.code);
  };
  window.addEventListener("keydown", record, true);
  await emit("pane-shortcut", "new");
  await ready(() => contentTabs().length === 1);
  const opening = contentTabs()[0]!;
  const animation = opening.getAnimations().find((entry) => entry.effect?.getTiming().duration === 200);
  seen.openAnimation = !!animation;
  if (animation) {
    animation.pause();
    animation.currentTime = 0;
    seen.openStart = opening.getBoundingClientRect().width;
    animation.currentTime = 100;
    seen.openMiddle = opening.getBoundingClientRect().width;
    animation.finish();
  }
  seen.openEnd = opening.getBoundingClientRect().width;
  seen.openStyle = { color: getComputedStyle(opening).color, fill: getComputedStyle(opening).backgroundColor, shadow: getComputedStyle(opening).boxShadow };
  await menu("new");
  await wait(220);
  const [first, second] = sessionTabs(id).tabs;
  await menu("previous-tab"); seen.previous = sessionTabs(id).active === first!.id;
  await menu("next-tab"); seen.next = sessionTabs(id).active === second!.id;
  await menu("tab-1"); seen.number = sessionTabs(id).active === "chat";
  await menu("cycle-next"); seen.cycle = sessionTabs(id).active === first!.id;
  await menu("cycle-previous"); seen.cycleBack = sessionTabs(id).active === "chat";
  await menu("next"); seen.bracket = sessionTabs(id).active === first!.id;
  await menu("previous"); seen.bracketBack = sessionTabs(id).active === "chat";
  await menu("tab-2");
  await menu("close-others");
  seen.kept = sessionTabs(id).tabs.map((tab) => tab.id);
  seen.expectedKept = first!.id;
  await wait(140);
  await menu("close");
  seen.closed = sessionTabs(id).tabs.length;
  const closing = bar().querySelector<HTMLElement>("[data-closing]");
  seen.closingRetained = !!closing;
  seen.closingInert = closing?.inert ?? false;
  seen.closeDuration = closing ? getComputedStyle(closing).transitionDuration : null;
  await ready(() => !bar().querySelector("[data-closing]") && bar().querySelectorAll('[role="tab"]').length === 1);
  seen.lastLeavesChat = bar().querySelector('[role="tab"]')?.closest("[data-tab-id]")?.getAttribute("data-tab-id") === "chat";
  await menu("reopen");
  seen.reopened = sessionTabs(id).active === first!.id;
  seen.forwarded = forwarded;
  window.removeEventListener("keydown", record, true);
  // Hover/active visibility of close controls is measured separately by pointer automation.
  return seen;
}
createRoot(document.getElementById("root")!).render(<TooltipProvider><Fixture /></TooltipProvider>);
