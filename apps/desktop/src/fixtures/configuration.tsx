/**
 * Settings → Configuration over a faked daemon: its rows, the explanations in its menus, and
 * search and old links landing on its rows.
 *
 * ?drive=1 reads the rows, opens the model and permission menus, follows an old page's link
 * and a search, and writes what it saw to #configuration-result
 * (ConfigurationPageRender.test.ts reads it). Without it the page stays as it opens, for
 * screenshots; ?menu=sessions | chats | permission opens that row's menu.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { useReveal } from "@/app/settings/reveal";
import { SettingsNav } from "@/app/settings/SettingsNav";
import { SettingsView } from "@/app/settings/SettingsView";
import { TooltipProvider } from "@/components/overlays";
import { OVERVIEWS } from "@/fixtures/agents";
import type { Node, Request } from "@/ipc/generated";
import { openSettings } from "@/state/actions";
import { useApp } from "@/state/store";

const params = new URLSearchParams(location.search);
if (params.has("drive")) {
  const still = document.createElement("style");
  still.textContent =
    "*, *::before, *::after { animation: none !important; transition: none !important; backdrop-filter: none !important; }";
  document.head.append(still);
}

useApp.setState((state) => ({
  info: { ...state.info, platform: "macos" } as typeof state.info,
  connection: { ...state.connection, status: "connected" } as typeof state.connection,
  settings: { ...state.settings, defaultOrchestrator: null, defaultChatModel: null },
  providers: { ...state.providers, view: { providers: OVERVIEWS, sessions: [], fixtures: [] } },
}));

const memory = (id: string, title: string) =>
  ({ id, title, body: "", updatedAtMs: Date.now(), provenance: { origin: "user", sessionId: null } }) as unknown as Node;
const memories = [memory("m1", "Lives in Lisbon"), memory("m2", "Works as an electrician")];

const calls: string[] = [];

mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  switch (req.method) {
    case "listMemories":
      return { method: req.method, memories };
    case "getBrigadeSkill":
      return { method: req.method, custom: true, path: "~/.brigadier/brigade/SKILL.md" };
    case "resetBrigadeSkill":
      calls.push("reset");
      return { method: req.method, custom: false, path: "~/.brigadier/brigade/SKILL.md" };
    case "updateSettings":
      return { method: req.method, settings: req.settings };
    default:
      return { method: req.method };
  }
});

const all = <T extends Element = HTMLElement>(selector: string, root: ParentNode = document) => [
  ...root.querySelectorAll<T>(selector),
];
const text = (element: Element | null | undefined) => element?.textContent?.trim() ?? "";
const trigger = (label: string) => document.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`);
const press = (element: Element | null | undefined) =>
  element?.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, button: 0 }));
const closeMenu = () =>
  document.activeElement?.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
/** The open menu's choices: their name, then the line under it. */
const menuChoices = () =>
  all('[role="menuitemradio"], [role="menuitemcheckbox"]').map((item) => {
    const lines = all(":scope > span > span", item).map(text).filter(Boolean);
    return lines.length > 0 ? lines : [text(item)];
  });
/** Whether the element is in the page's scrolled view. */
const inView = (element: Element | undefined | null) => {
  if (!element) return false;
  const box = element.getBoundingClientRect();
  return box.bottom > 0 && box.top < window.innerHeight;
};
const typeSearch = (value: string) => {
  const input = document.querySelector<HTMLInputElement>('input[aria-label="Search settings"]');
  if (!input) return;
  Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set?.call(input, value);
  input.dispatchEvent(new Event("input", { bubbles: true }));
  input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
};
const skill = () => document.querySelector('[data-setting="How sessions run workers"]');
const MENUS: Record<string, string> = {
  sessions: "Model for sessions",
  chats: "Model for chats",
  permission: "Default permission level",
};

function Fixture() {
  const page = useApp((s) => (s.selection.type === "settings" ? s.selection.page : null));
  const [result, setResult] = useState("");
  useEffect(() => {
    openSettings("configuration");
    const menu = params.get("menu");
    if (menu && MENUS[menu]) {
      const timer = setTimeout(() => press(trigger(MENUS[menu]!)), 1200);
      return () => clearTimeout(timer);
    }
    if (!params.has("drive")) return;
    const seen: Record<string, unknown> = {};
    const steps: (() => void)[] = [
      () => {
        const [first] = all('[data-slot="settings-card"]');
        seen.firstCard = all('[data-slot="settings-row"]', first).map((row) => ({
          setting: row.dataset.setting,
          helper: text(row.querySelector(".text-xs")),
        }));
        seen.firstCardHeading = first?.closest("section") !== null;
        seen.sections = all("main section h2").map(text);
        seen.buttons = all("main button").map(text).filter((name) => name === "Use automatic");
        seen.levelRow = all("[data-setting]").some((row) => text(row).endsWith("Default"));
        seen.triggers = Object.values(MENUS).map((label) => text(trigger(label)));
        seen.skill = { helper: text(skill()?.querySelector(".text-xs")), button: text(skill()?.querySelector("button")) };
        skill()?.querySelector("button")?.click();
      },
      () => {
        seen.skillReset = { helper: text(skill()?.querySelector(".text-xs")), buttons: all("button", skill() ?? document.body).length, calls };
        press(trigger(MENUS.sessions!));
      },
      () => {
        seen.sessionsMenu = menuChoices().slice(0, 2);
        closeMenu();
      },
      () => press(trigger(MENUS.chats!)),
      () => {
        seen.chatsMenu = menuChoices().slice(0, 1);
        closeMenu();
      },
      () => press(trigger(MENUS.permission!)),
      () => {
        seen.permissionMenu = menuChoices();
        closeMenu();
        openSettings("git");
      },
      // A page since merged into Configuration opens it at its row.
      () => openSettings("personalization"),
      () => {
        const row = document.querySelector('[data-setting="Memories"]');
        seen.oldLink = { page: useApp.getState().selection, inView: inView(row) };
        openSettings("git");
      },
      () => typeSearch("Full access"),
      () => {
        seen.search = { page: useApp.getState().selection, inView: inView(document.querySelector('[data-setting="Default permission level"]')) };
        setResult(JSON.stringify(seen));
      },
    ];
    let stopped = false;
    const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
    void (async () => {
      await wait(500);
      for (const step of steps) {
        if (stopped) return;
        step();
        await wait(300);
        // A reveal waits for frames, which a headless page may give slowly.
        for (let tries = 0; useReveal.getState().label !== null && tries < 30; tries++) await wait(100);
      }
    })();
    return () => {
      stopped = true;
    };
  }, []);
  return (
    <TooltipProvider>
      <div className="bg-background text-foreground flex h-screen">
        <aside className="w-64 shrink-0">
          <SettingsNav />
        </aside>
        <main className="flex h-full min-w-0 flex-1">
          <div className="min-w-0 flex-1">{page && <SettingsView page={page} />}</div>
        </main>
      </div>
      {result && <pre id="configuration-result">{result}</pre>}
    </TooltipProvider>
  );
}

const root = document.getElementById("root");
if (root) createRoot(root).render(<Fixture />);
