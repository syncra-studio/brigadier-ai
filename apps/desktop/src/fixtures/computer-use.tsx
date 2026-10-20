/** Settings › Computer use over a faked daemon. */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { ComputerUsePage } from "@/app/settings/ComputerUsePage";
import { TooltipProvider } from "@/components/overlays/tooltip";
import type { ComputerSetup, ComputerUseStatus, PermissionState, Request, Settings } from "@/ipc/generated";
import { onComputerUse } from "@/state/computerUse";
import { useApp } from "@/state/store";

/*
 * ?drive=1 shows the page in each permission and setup state, presses its buttons, and writes
 * what it saw to #computer-use-result (ComputerUsePageRender.test.ts reads it).
 */
const params = new URLSearchParams(location.search);
if (params.has("drive")) {
  const still = document.createElement("style");
  still.textContent = "*, *::before, *::after { animation: none !important; transition: none !important; }";
  document.head.append(still);
}

useApp.setState((state) => ({ info: { ...state.info, platform: "macos" } as typeof state.info }));

const status = (
  setup: ComputerSetup,
  accessibility: PermissionState = "unknown",
  screenRecording: PermissionState = "unknown",
  current = false,
): ComputerUseStatus => ({ available: true, accessibility, screenRecording, current, setup, sessions: [] });

const calls: string[] = [];
mockIPC((command, payload) => {
  if (command === "open_privacy_settings" || command === "open_computer_setup") {
    const permission = (payload as { permission?: string } | undefined)?.permission;
    calls.push(permission ? `${command} ${permission}` : command);
    return null;
  }
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  switch (req.method) {
    case "getComputerUse":
      calls.push(req.method);
      // Never set up: the helper has never run.
      return { method: req.method, computerUse: status({ state: "notSetUp" }) };
    case "grantComputerPermission":
      calls.push(`${req.method} ${req.permission}`);
      return { method: req.method };
    case "updateSettings":
      calls.push(req.method);
      return { method: req.method, settings: req.settings as Settings };
    default:
      calls.push(req.method);
      return { method: req.method };
  }
});

const all = <T extends Element = HTMLElement>(selector: string, root: ParentNode = document) => [
  ...root.querySelectorAll<T>(selector),
];
const text = (element: Element | null | undefined) => element?.textContent?.trim() ?? "";
const row = (label: string) => all('[data-slot="settings-row"]').find((element) => element.dataset.setting === label);
const button = (name: string, root: ParentNode = document) =>
  all<HTMLButtonElement>("button", root).find((element) => text(element) === name || element.getAttribute("aria-label") === name);
const buttons = (root: ParentNode | undefined) => (root ? all<HTMLButtonElement>("button", root).map(text) : []);

/** What the page shows now. */
function look() {
  const permission = (label: string) => {
    const found = row(label);
    return {
      state: text(found?.querySelector('[data-slot="permission-state"]')),
      buttons: buttons(found),
      grantDisabled: button("Grant…", found ?? document)?.disabled ?? null,
    };
  };
  return {
    accessibility: permission("Accessibility"),
    screenRecording: permission("Screen Recording"),
    note: text(all("main section").find((section) => text(section.querySelector("h2")) === "Permissions")?.querySelector("h2 + p")),
    setup: text(document.querySelector('[data-slot="setup-line"]')),
    setupButtons: buttons(row("Setup")),
    menuBarDisabled: all<HTMLButtonElement>('[role="switch"]', row("Show in menu bar"))[0]?.disabled ?? null,
  };
}

function Fixture() {
  const [result, setResult] = useState("");
  useEffect(() => {
    if (!params.has("drive")) return;
    const seen: Record<string, unknown> = {};
    const show = (name: string, next: ComputerUseStatus) => [
      () => onComputerUse(next),
      () => {
        seen[name] = look();
      },
    ];
    const steps: (() => void)[] = [
      () => {
        seen.sections = all("main section h2").map(text);
        seen.rows = all('[data-slot="settings-row"]').map((element) => element.dataset.setting);
        seen.first = look();
        seen.listingCalls = [...calls];
      },
      ...show("unknown", status({ state: "notSetUp" })),
      ...show("cached", status({ state: "missing" }, "granted", "notGranted")),
      ...show("missingBoth", status({ state: "missing" }, "notGranted", "notGranted", true)),
      ...show("missingAccessibility", status({ state: "missing" }, "notGranted", "granted", true)),
      ...show("confirmCapture", status({ state: "confirmCapture" }, "granted", "granted", true)),
      ...show("ready", status({ state: "ready" }, "granted", "granted", true)),
      ...show("readyCached", status({ state: "ready" }, "granted", "granted", false)),
      ...show("restarting", status({ state: "restarting" }, "granted", "granted", false)),
      ...show("failed", status({ state: "failed", message: "the helper app is missing from this install" }, "granted", "granted")),
      ...show("beforeButtons", status({ state: "missing" }, "notGranted", "notGranted", true)),
      () => button("Grant…", row("Screen Recording"))?.click(),
      () => button("Open System Settings for Accessibility")?.click(),
      () => button("Finish setup…")?.click(),
      () => {
        useApp.setState((state) => ({ settings: { ...state.settings, computerUse: false } }));
      },
      ...show("off", status({ state: "off" }, "granted", "notGranted", false)),
      () => {
        seen.calls = calls;
        setResult(JSON.stringify(seen));
      },
    ];
    let stopped = false;
    const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
    void (async () => {
      await wait(400);
      for (const step of steps) {
        if (stopped) return;
        step();
        await wait(120);
      }
    })();
    return () => {
      stopped = true;
    };
  }, []);
  return (
    <TooltipProvider>
      <div className="bg-background text-foreground flex h-screen">
        <main className="flex h-full min-w-0 flex-1">
          <div className="min-w-0 flex-1">
            <ComputerUsePage />
          </div>
        </main>
      </div>
      {result && <pre id="computer-use-result">{result}</pre>}
    </TooltipProvider>
  );
}

const root = document.getElementById("root");
if (root) createRoot(root).render(<Fixture />);
