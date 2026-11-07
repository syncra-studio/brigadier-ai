/** Settings › Computer use over a faked daemon. */
import { mockIPC } from "@/ipc/nativeMocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { ComputerUsePage } from "@/app/settings/ComputerUsePage";
import { TooltipProvider } from "@/components/overlays";
import { account, model, overview, status as providerStatus } from "@/fixtures/agents";
import { usageView } from "@/fixtures/providersData";
import type { ComputerSetup, ComputerUseStatus, GuidanceRow, ModelRules, PermissionState, Request, Settings } from "@/ipc/generated";
import { IDLE_RESEARCH } from "@/fixtures/rankings";
import { onComputerUse } from "@/state/computerUse";
import { loadModelRules, useModelRules } from "@/state/modelRules";
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

const accounts = [account("claude", undefined, "claude@example.com"), account("codex", undefined, "codex@example.com")];
useApp.setState((state) => ({
  info: { ...state.info, platform: "macos" } as typeof state.info,
  accounts: { accounts, warnings: [] },
  providers: { ...state.providers, view: { providers: [
    overview("claude", [model("opus", "Opus", { isDefault: true, defaultEffort: "high" })]),
    overview("codex", [model("gpt-6-luna", "Gpt-6-luna", { isDefault: true, defaultEffort: "low" })]),
  ], sessions: [], fixtures: [] } },
}));

const status = (
  setup: ComputerSetup,
  accessibility: PermissionState = "unknown",
  screenRecording: PermissionState = "unknown",
  current = false,
): ComputerUseStatus => ({ available: true, platform: "macos", accessibility, screenRecording, current, setup, sessions: [], notes: [] });

// The rules file: the shipped "Computer use" order and nothing of the user's yet.
const cuRow = (agent: GuidanceRow["agent"], modelId: string, effort: string | null): GuidanceRow => ({ work: "Computer use", agent, model: modelId, effort, reason: "Shipped." });
const modelRules: ModelRules = { path: "/tmp/models.md", revision: "fixture", shipped: [cuRow("claude", "opus", "high"), cuRow("codex", "default", "high")], rows: [], rules: [], suggestions: [], research: IDLE_RESEARCH };

const calls: string[] = [];
const modelSaves: Request[] = [];
// The first check's answer, held until the fixture has seen the page while it runs.
let answerCheck: () => void = () => {};
mockIPC((command, payload) => {
  if (command === "open_privacy_settings" || command === "open_computer_setup") {
    const permission = (payload as { permission?: string } | undefined)?.permission;
    calls.push(permission ? `${command} ${permission}` : command);
    return null;
  }
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  switch (req.method) {
    // The model row's reads, outside what the page's own calls record.
    case "getModelRules":
      return { method: req.method, rules: modelRules };
    case "getUsage":
      return { method: req.method, usage: usageView() };
    case "setModelRules":
      modelSaves.push(req);
      return { method: req.method, rules: { ...modelRules, rows: req.rows, rules: req.rules } };
    case "getComputerUse":
      calls.push(req.method);
      // Never set up: the helper has never run.
      return { method: req.method, computerUse: status({ state: "notSetUp" }) };
    case "checkComputerUse":
      calls.push(req.method);
      // The helper holds both permissions.
      return new Promise((resolve) => {
        answerCheck = () => {
          const computerUse = status({ state: "confirmCapture" }, "granted", "granted", true);
          onComputerUse(computerUse);
          resolve({ method: req.method, computerUse });
        };
      });
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

void loadModelRules();

const all = <T extends Element = HTMLElement>(selector: string, root: ParentNode = document) => [
  ...root.querySelectorAll<T>(selector),
];
const text = (element: Element | null | undefined) => element?.textContent?.trim() ?? "";
const row = (label: string) => all('[data-slot="settings-row"]').find((element) => element.dataset.setting === label);
const button = (name: string, root: ParentNode = document) =>
  all<HTMLButtonElement>("button", root).find((element) => text(element) === name || element.getAttribute("aria-label") === name);
const pickModel = (name: string) => all<HTMLButtonElement>('[data-slot="model-picker-model"]').find((element) => text(element).startsWith(name))?.click();
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

/** The model row's choice as its pill shows it, and whether it can be opened. */
function modelLook() {
  const pill = document.querySelector<HTMLButtonElement>('[data-slot="computer-use-model"]');
  return { shown: text(pill), disabled: pill?.disabled ?? null };
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
        seen.model = modelLook();
      },
      // A signed-out Claude is skipped in favor of Codex, as worker starts do.
      () => useApp.setState({ accounts: { accounts: accounts.map((entry) => entry.account.provider === "claude"
        ? { ...entry, status: providerStatus("claude", { loggedIn: false }) } : entry), warnings: [] } }),
      () => { seen.modelCodexOnly = modelLook(); },
      // An extra account can lead even while the computer's own login is signed out.
      () => useApp.setState((state) => ({ accounts: { accounts: [
        ...state.accounts!.accounts.map((entry) => entry.account.provider === "claude" ? { ...entry, inUse: false } : entry),
        account("claude", "extra", "extra@example.com", { inUse: true }),
      ], warnings: [] } })),
      () => { seen.modelExtraAccount = modelLook(); },
      () => useApp.setState({ accounts: { accounts, warnings: [] } }),
      // The rules refuse a picked model before anything reaches the file.
      () => useModelRules.setState({ view: { ...modelRules, rules: [{ type: "forbid", names: ["opus"], work: "Computer use" }] } }),
      () => button("Model for computer use")?.click(),
      () => document.querySelector<HTMLButtonElement>('[data-slot="model-picker-agent"][data-agent="claude"]')?.click(),
      () => pickModel("Opus"),
      () => {
        seen.modelForbidden = {
          saves: modelSaves.length,
          error: text(row("Model for computer use")?.querySelector('[role="alert"]')),
          greyed: all('[data-slot="model-picker-model"]').find((element) => text(element).startsWith("Opus"))?.dataset.refused,
        };
      },
      // A pick keeps the picker open: its trigger closes it.
      () => button("Model for computer use")?.click(),
      // Moving an existing model first keeps its effort: judge that effort before saving.
      () => useModelRules.setState({ view: { ...modelRules, rules: [{ type: "effortCeiling", effort: "low", work: "Computer use" }] } }),
      () => button("Model for computer use")?.click(),
      () => document.querySelector<HTMLButtonElement>('[data-slot="model-picker-agent"][data-agent="claude"]')?.click(),
      () => pickModel("Opus"),
      () => {
        seen.modelAboveCeiling = { saves: modelSaves.length, error: text(row("Model for computer use")?.querySelector('[role="alert"]')) };
      },
      () => button("Model for computer use")?.click(),
      // A model the user picked on either page: their own row of the work.
      () => useModelRules.setState({ view: { ...modelRules, rows: [cuRow("codex", "gpt-6-luna", "low")] } }),
      () => {
        seen.modelMine = modelLook();
      },
      () => answerCheck(),
      () => {
        seen.checked = look();
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
        seen.calls = [...calls];
      },
      // Linux: nothing to grant; the page says what the desktop session allows.
      () => {
        useApp.setState((state) => ({
          info: { ...state.info, platform: "linux" } as typeof state.info,
          settings: { ...state.settings, computerUse: true },
        }));
        onComputerUse({
          ...status({ state: "ready" }, "granted", "granted", true),
          platform: "linux",
          notes: ["Wayland session: apps running under XWayland work in the background."],
        });
      },
      () => {
        seen.linux = {
          sections: all("main section h2").map(text),
          rows: all('[data-slot="settings-row"]').map((element) => element.dataset.setting),
          notes: text(document.querySelector('[data-slot="desktop-notes"]')),
          setup: text(document.querySelector('[data-slot="setup-line"]')),
          setupButtons: buttons(row("Setup")),
        };
      },
      ...(["windows", "linux"] as const).flatMap((platform) => [
        () => {
          useApp.setState((state) => ({ info: { ...state.info, platform } as typeof state.info }));
          onComputerUse({ ...status({ state: "ready" }, "granted", "granted", true), platform });
        },
        () => { seen[platform + "NoNotes"] = text(document.querySelector('[data-slot="desktop-notes"]')); },
        () => { onComputerUse({ ...status({ state: "ready" }, "granted", "granted", false), platform }); },
        () => { seen[platform + "CachedNoNotes"] = text(document.querySelector('[data-slot="desktop-notes"]')); },
        () => { onComputerUse({ ...status({ state: "off" }, "granted", "granted", true), platform }); },
        () => { seen[platform + "OffNoNotes"] = text(document.querySelector('[data-slot="desktop-notes"]')); },
      ]),
      () => {
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
