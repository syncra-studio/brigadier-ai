/** Settings → Providers over a faked daemon: accounts with masked emails, models, rules. */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { SettingsNav } from "@/app/settings/SettingsNav";
import { SettingsView } from "@/app/settings/SettingsView";
import { TooltipProvider } from "@/components/ui/tooltip";
import type {
  AccountView,
  ModelInfo,
  OverrideRule,
  ProviderKind,
  ProviderOverview,
  ProviderStatus,
  Request,
} from "@/ipc/generated";
import { openSettings } from "@/state/actions";
import { useApp } from "@/state/store";

/*
 * ?drive=1 reveals one account's email, leaves the page and comes back, opens a search result
 * whose row moved, and follows a new model's link to its rule, writing what it saw to
 * #providers-result (ProvidersPageRender.test.ts reads it). Without it, the page stays as it
 * opens, for screenshots.
 */
const params = new URLSearchParams(location.search);
// Driven, nothing animates or blurs (see the archived chats fixture).
if (params.has("drive")) {
  const still = document.createElement("style");
  still.textContent =
    "*, *::before, *::after { animation: none !important; transition: none !important; backdrop-filter: none !important; }";
  document.head.append(still);
}

function model(id: string, displayName: string, fields: Partial<ModelInfo> = {}): ModelInfo {
  return {
    id,
    displayName,
    description: "",
    resolved: null,
    efforts: [],
    defaultEffort: null,
    isDefault: false,
    inputModalities: [],
    fast: null,
    legacy: false,
    ...fields,
  };
}

function status(provider: ProviderKind, fields: Partial<ProviderStatus> = {}): ProviderStatus {
  return {
    provider,
    path: `/usr/local/bin/${provider}`,
    version: "2.1.0",
    loggedIn: true,
    authMethod: "subscription",
    plan: "max",
    guidance: null,
    compacts: true,
    ...fields,
  };
}

function overview(provider: ProviderKind, models: ModelInfo[]): ProviderOverview {
  return {
    provider,
    status: status(provider),
    models: { provider, models, cliVersion: "2.1.0", fetchedAtMs: 0 },
    quota: null,
    usage: null,
    error: null,
    checkedAtMs: Date.now(),
  };
}

const quota = (provider: ProviderKind, fiveHour: number, week: number) => ({
  provider,
  windows: [
    { id: "five_hour", label: "5-hour", usedPercent: fiveHour, resetsAtMs: null, windowMinutes: 300, bucket: null, model: null },
    { id: "seven_day", label: "Weekly", usedPercent: week, resetsAtMs: null, windowMinutes: 10_080, bucket: null, model: null },
  ],
  limit: null,
  observedAtMs: Date.now(),
  source: "read" as const,
});

function account(provider: ProviderKind, id: string | undefined, fields: Partial<AccountView>, email: string): AccountView {
  return {
    account: id === undefined ? { provider } : { provider, account: id },
    name: "",
    default: false,
    status: status(provider, { email }),
    quota: quota(provider, 32, 69),
    quotaAtMs: Date.now(),
    checkedAtMs: Date.now(),
    checking: false,
    error: null,
    ...fields,
  };
}

const accounts: AccountView[] = [
  account("claude", undefined, { default: true }, "stephen.golban@example.com"),
  account("claude", "acct-work", { name: "work.person@corp.example.io", quota: quota("claude", 84, 93) }, "work.person@corp.example.io"),
  account("claude", "acct-side", { name: "Side project" }, "side@example.org"),
  account("codex", undefined, { default: true }, "stephen@example.com"),
];

const workerKinds: OverrideRule["categories"] = ["scout", "research", "implement", "review", "merge", "verify", "operate"];
const rules: OverrideRule[] = [
  // What Routing's old switch wrote for a model kept from tasks.
  { id: "old-switch", effect: "never", target: { type: "model", provider: "claude", id: "sonnet" }, categories: workerKinds, areas: [], projectId: null, createdAtMs: 1 },
  // A model seen after its agent's first list.
  { id: "new-model-0192", effect: "never", target: { type: "model", provider: "codex", id: "gpt-6-luna" }, categories: workerKinds, areas: [], projectId: null, createdAtMs: 2 },
];

useApp.setState((state) => ({
  info: { ...state.info, platform: "macos" } as typeof state.info,
  connection: { ...state.connection, status: "connected" } as typeof state.connection,
  settings: { ...state.settings, routingOverrides: rules, switchAccounts: true },
  accounts: { accounts },
  providers: {
    ...state.providers,
    view: {
      providers: [
        overview("claude", [
          model("opus", "Opus 5.5", { isDefault: true }),
          model("sonnet", "Sonnet 5.5"),
          model("claude-fable-5-1", "Fable 5.1"),
        ]),
        overview("codex", [model("gpt-6", "GPT-6", { isDefault: true }), model("gpt-6-luna", "GPT-6 Luna")]),
      ],
      sessions: [],
      fixtures: [],
    },
  },
}));

mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  switch (req.method) {
    case "getAccounts":
      return { method: req.method, accounts: { accounts } };
    case "updateSettings":
      return { method: req.method, settings: req.settings };
    case "getRankingsRefresh":
      return { method: req.method, refresh: null };
    case "previewRoutes":
      return { method: req.method, routes: [] };
    default:
      return { method: req.method };
  }
});

const all = <T extends Element = HTMLElement>(selector: string, root: ParentNode = document) => [
  ...root.querySelectorAll<T>(selector),
];
const text = (element: Element | null | undefined) => element?.textContent?.trim() ?? "";
const button = (name: string, root: ParentNode = document) =>
  all<HTMLButtonElement>("button", root).find(
    (element) => text(element) === name || element.getAttribute("aria-label") === name,
  );
const titles = () => all('[data-slot="account-title"]').map(text);
const advancedOpen = () => document.querySelector('[data-slot="settings-advanced"]')?.getAttribute("data-state") === "open";
/** Whether the element is in the page's scrolled view. */
const inView = (element: Element | undefined | null) => {
  if (!element) return false;
  const box = element.getBoundingClientRect();
  return box.bottom > 0 && box.top < window.innerHeight;
};
const section = (name: string) =>
  all("section").find((element) => {
    const id = element.getAttribute("aria-labelledby");
    return id !== null && text(document.getElementById(id)) === name;
  });
const typeSearch = (value: string) => {
  const input = document.querySelector<HTMLInputElement>('input[aria-label="Search settings"]');
  if (!input) return;
  Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set?.call(input, value);
  input.dispatchEvent(new Event("input", { bubbles: true }));
  input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
};

function Fixture() {
  const page = useApp((s) => (s.selection.type === "settings" ? s.selection.page : null));
  const [result, setResult] = useState("");
  useEffect(() => {
    openSettings("providers");
    if (!params.has("drive")) return;
    const seen: Record<string, unknown> = {};
    const steps: (() => void)[] = [
      () => {
        seen.sections = all("main section > div h2").map(text);
        seen.switchRow = Boolean(document.querySelector('[data-setting="Switch to another account when one runs out"]'));
        seen.masked = titles();
        seen.eyes = all("button").filter((element) => element.getAttribute("aria-label") === "Show email").length;
        seen.usage = all('[data-slot="account-usage"]').slice(0, 4).map((element) => ({
          text: text(element),
          tone: element.lastElementChild?.className,
        }));
        seen.models = all('[data-slot="settings-card"]')
          .flatMap((card) => all('[data-slot="settings-row"]', card))
          .filter((row) => row.querySelector('button[role="switch"][aria-label^="Use "]') && !row.dataset.setting)
          .map(text);
        seen.advancedAtFirst = advancedOpen();
        // Reveal the second account's email only.
        all<HTMLButtonElement>("button").filter((element) => element.getAttribute("aria-label") === "Show email")[1]?.click();
      },
      () => {
        seen.revealed = titles();
        openSettings("git");
      },
      () => {
        openSettings("providers");
      },
      () => {
        seen.afterLeaving = titles();
        typeSearch("Models Brigadier can give work to");
      },
      () => {
        seen.searched = {
          page: useApp.getState().selection,
          advanced: advancedOpen(),
          rulesInView: inView(section("Rules")),
          rules: all('[data-setting^="Don\'t give"]').map((row) => text(row)),
        };
        openSettings("git");
      },
      () => {
        openSettings("providers");
      },
      () => {
        seen.advancedAgain = advancedOpen();
        const allow = button("Allow in Rules");
        seen.newModelRow = text(allow?.closest('[data-slot="settings-row"]'));
        allow?.click();
      },
      () => {
        const rule = all('[data-setting^="Don\'t give"]').find((row) => row.dataset.setting?.includes("Luna"));
        seen.allowed = { advanced: advancedOpen(), rule: rule?.dataset.setting, inView: inView(rule) };
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
        await wait(250);
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
      {result && <pre id="providers-result">{result}</pre>}
    </TooltipProvider>
  );
}

const root = document.getElementById("root");
if (root) createRoot(root).render(<Fixture />);
