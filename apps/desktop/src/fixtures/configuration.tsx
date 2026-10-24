/**
 * Settings → Configuration over a faked daemon: its rows, the explanations in its menus, and
 * search and old links landing on its rows.
 *
 * ?drive=1 reads the rows, picks a worker count, opens the model and permission menus, follows an old page's link
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
import type { Request } from "@/ipc/generated";
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

const calls: string[] = [];

mockIPC((command, payload) => {
  if (command === "reveal_path") {
    calls.push(`show ${(payload as { path: string }).path}`);
    return null;
  }
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  switch (req.method) {
    case "getBrigadeSkill":
      return { method: req.method, custom: true, path: "~/.brigadier/brigade/SKILL.md" };
    case "resetBrigadeSkill":
      calls.push("reset");
      return { method: req.method, custom: false, path: "~/.brigadier/brigade/SKILL.md" };
    case "createBrigadeSkill":
      calls.push("create");
      return { method: req.method, custom: true, path: "~/.brigadier/brigade/SKILL.md" };
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
/** The name a screen reader reads: its labelledby elements' labels or words, else its label. */
const accessibleName = (element: Element | null) => {
  const ids = element?.getAttribute("aria-labelledby")?.split(" ") ?? [];
  if (ids.length === 0) return element?.getAttribute("aria-label") ?? "";
  return ids.map((id) => {
    const named = document.getElementById(id);
    return named?.getAttribute("aria-label") ?? text(named);
  }).join(" ");
};
const closeMenu = () =>
  document.activeElement?.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
/** The open menu's or model picker's choices: their name, then the line under it. */
const menuChoices = () =>
  all('[role="menuitemradio"], [role="menuitemcheckbox"], [data-picker-part="models"] [role="option"]').map((item) => {
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
const skillButtons = () => all("button", skill() ?? document.createElement("div"));
const skillButton = (name: string) => skillButtons().find((button) => text(button) === name);
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
      const timer = setTimeout(() => trigger(MENUS[menu]!)?.click(), 1200);
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
        // Each picker says the choice it holds (#564).
        seen.names = Object.values(MENUS).map((label) => accessibleName(trigger(label)));
        seen.skill = { helper: text(skill()?.querySelector(".text-xs")), buttons: skillButtons().map(text) };
        skillButton("Reset to shipped")?.click();
      },
      () => {
        seen.skillReset = { helper: text(skill()?.querySelector(".text-xs")), buttons: skillButtons().map(text), calls: [...calls] };
        skillButton("Create")?.click();
      },
      () => {
        seen.skillCreated = { helper: text(skill()?.querySelector(".text-xs")), buttons: skillButtons().map(text), calls: [...calls] };
        skillButton("Show file")?.click();
      },
      () => {
        seen.skillShown = [...calls];
        const row = document.querySelector('[data-setting="Workers at once"]');
        seen.workers = { helper: text(row?.querySelector(".text-xs")), choice: text(trigger("Workers at once")) };
        trigger("Workers at once")?.click();
      },
      () => {
        seen.workersMenu = menuChoices().map((choice) => choice[0]);
        all('[role="menuitemradio"]').find((item) => text(item) === "5")?.click();
      },
      () => {
        seen.workersPicked = { choice: text(trigger("Workers at once")), saved: useApp.getState().settings.workersAtOnce };
        trigger(MENUS.sessions!)?.click();
      },
      () => {
        seen.sessionsMenu = menuChoices().slice(0, 2);
        closeMenu();
      },
      // A click alone, as an accessibility press (VoiceOver's) is, opens a menu too (#564).
      () => trigger(MENUS.chats!)?.click(),
      () => {
        seen.chatsMenu = menuChoices().slice(0, 1);
        closeMenu();
      },
      () => trigger(MENUS.permission!)?.click(),
      () => {
        seen.permissionMenu = menuChoices();
        closeMenu();
        openSettings("git");
      },
      // A page since merged into Configuration opens it at its row.
      () => openSettings("conversations"),
      () => {
        const row = document.querySelector('[data-setting="Model for sessions"]');
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
