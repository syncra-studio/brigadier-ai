/**
 * Settings → General over a faked shell and daemon: its five sections, the notification status
 * row and the rows it dims, Open at login and the licenses.
 *
 * ?notifications=off | notAsked | allowed is what the system says (off by default), and
 * ?login=off | on | needsApproval | unsupported how Open at login stands (off by default).
 * ?drive=1 reads the rows, chooses ⌘Enter and Steer, turns "needs you" off and Open at login on
 * and off, presses Allow (when not asked yet) and View, and writes what it saw to
 * #general-result (GeneralPageRender.test.ts reads it). Without it the page stays as it opens,
 * for screenshots; ?licenses=1 opens the licenses.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { SettingsNav } from "@/app/settings/SettingsNav";
import { SettingsView } from "@/app/settings/SettingsView";
import { SidebarProvider } from "@/components/layout";
import { TooltipProvider } from "@/components/overlays";
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
  info: {
    ...state.info,
    platform: "macos",
    version: "0.9.0",
    build: { sha: "a1b2c3d4e", date: "2026-10-16" },
    updateChannel: "Nightly",
  } as typeof state.info,
  connection: { ...state.connection, status: "connected" } as typeof state.connection,
  windowVisible: true,
}));

/** What the page asked the shell and the daemon, in order. */
const calls: string[] = [];
let permission = params.get("notifications") ?? "off";
let login = params.get("login") ?? "off";
let updateChecks = 0;
/** The settings as last saved, to tell what a save changed. */
let saved = useApp.getState().settings;

mockIPC((command, payload) => {
  switch (command) {
    case "notification_permission":
      return permission;
    case "ask_notification_permission":
      calls.push("askNotifications");
      permission = "allowed";
      return permission;
    case "open_notification_settings":
      calls.push("openNotificationSettings");
      return null;
    case "login_item":
      return login;
    case "set_login_item": {
      const on = (payload as { on: boolean }).on;
      calls.push(`setLoginItem ${on}`);
      login = on ? "on" : "off";
      return login;
    }
    case "open_login_items":
      calls.push("openLoginItems");
      return null;
    case "check_for_updates":
      calls.push("checkForUpdates");
      // The second check finds the feed unreachable (offline).
      if (++updateChecks > 1) throw new Error("the update feed didn't answer");
      return null;
    case "ipc_request": {
      const req = (payload as { request: Request }).request;
      switch (req.method) {
        case "updateSettings": {
          const changed = Object.entries(req.settings).filter(
            ([key, value]) => saved[key as keyof typeof saved] !== value,
          );
          for (const [key, value] of changed) calls.push(`${key} ${String(value)}`);
          saved = req.settings;
          return { method: req.method, settings: req.settings };
        }
        case "getKeepAwake":
          return {
            method: req.method,
            status: { active: false, screenOn: false, lidClosed: "ready" },
          };
        default:
          return { method: req.method };
      }
    }
    default:
      return null;
  }
});

const all = <T extends Element = HTMLElement>(selector: string, root: ParentNode = document) => [
  ...root.querySelectorAll<T>(selector),
];
const text = (element: Element | null | undefined) => element?.textContent?.trim() ?? "";
const row = (setting: string) => document.querySelector<HTMLElement>(`[data-setting="${setting}"]`);
const control = (label: string) => document.querySelector<HTMLElement>(`[aria-label="${label}"]`);
const button = (name: string) => all<HTMLButtonElement>("button").find((item) => text(item) === name);

function Fixture() {
  const page = useApp((s) => (s.selection.type === "settings" ? s.selection.page : null));
  const [result, setResult] = useState<string | null>(null);
  useEffect(() => {
    openSettings("general");
  }, []);
  useEffect(() => {
    if (params.has("licenses")) {
      const timer = setTimeout(() => button("View")?.click(), 400);
      return () => clearTimeout(timer);
    }
    if (!params.has("drive")) return;
    const seen: Record<string, unknown> = {};
    const steps: (() => void)[] = [
      () => {
        seen.sections = all("main section h2").map(text);
        seen.rows = all<HTMLElement>("main [data-slot='settings-row']").map((item) => ({
          setting: item.dataset.setting,
          helper: text(item.querySelector(".text-xs")),
          dimmed: item.classList.contains("opacity-50"),
        }));
        seen.version = text(row("Version")?.querySelector("span.tabular-nums"));
        seen.build = text(row("Build")?.querySelector("span.tabular-nums"));
        seen.updatesBefore = text(row("Updates")?.querySelector(".text-xs"));
        seen.loginBefore = control("Open at login")?.getAttribute("aria-checked");
      },
      () => {
        seen.followUpHelper = text(row("Follow-up behaviour")?.querySelector(".text-xs"));
        all<HTMLElement>('[role="radio"]', row("Follow-up behaviour") ?? document).find((item) => text(item) === "Steer")?.click();
      },
      () => (control("Plain text composer") as HTMLButtonElement | null)?.click(),
      () => (control("When something needs you") as HTMLButtonElement | null)?.click(),
      () => (control("Open at login") as HTMLButtonElement | null)?.click(),
      () => {
        seen.loginOn = control("Open at login")?.getAttribute("aria-checked");
        (control("Open at login") as HTMLButtonElement | null)?.click();
      },
      () => {
        seen.loginOff = control("Open at login")?.getAttribute("aria-checked");
        (button("Allow") ?? button("Open System Settings"))?.click();
      },
      () => button("Check now")?.click(),
      () => {
        seen.updatesAfter = text(row("Updates")?.querySelector(".text-xs"));
      },
      () => button("Check now")?.click(),
      () => {
        seen.updatesFailed = text(row("Updates")?.querySelector(".text-xs"));
        seen.checkAgain = !(button("Check now") as HTMLButtonElement | null)?.disabled;
      },
      () => button("View")?.click(),
      () => {
        const dialog = document.querySelector('[role="dialog"]');
        seen.licenses = { title: text(dialog?.querySelector("h2")), mentions: text(dialog).includes("assistant-ui") };
      },
      () => {
        const notifications = all<HTMLElement>("main section")[2];
        seen.notificationsAfter = all<HTMLElement>("[data-slot='settings-row']", notifications ?? document).map(
          (item) => `${item.dataset.setting ?? ""}${item.classList.contains("opacity-50") ? " (dimmed)" : ""}`,
        );
        seen.followUp = all('[role="radio"][data-state="on"]', row("Follow-up behaviour") ?? document).map(text);
        seen.calls = calls;
        setResult(JSON.stringify(seen));
      },
    ];
    let stopped = false;
    const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
    void (async () => {
      await wait(600);
      for (const step of steps) {
        if (stopped) return;
        step();
        await wait(300);
      }
    })();
    return () => {
      stopped = true;
    };
  }, []);
  return (
    <TooltipProvider>
      <SidebarProvider>
        <div className="bg-background text-foreground flex h-screen w-full">
          <aside className="w-64 shrink-0">
            <SettingsNav />
          </aside>
          <main className="flex h-full min-w-0 flex-1">
            <div className="min-w-0 flex-1">{page && <SettingsView page={page} />}</div>
          </main>
        </div>
      </SidebarProvider>
      {result && <pre id="general-result">{result}</pre>}
    </TooltipProvider>
  );
}

const root = document.getElementById("root");
if (root) createRoot(root).render(<Fixture />);
