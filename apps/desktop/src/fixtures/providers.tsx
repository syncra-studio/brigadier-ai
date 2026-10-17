/** Settings → Providers over a faked daemon: a tab per agent, accounts with masked emails, models. */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { revealSetting } from "@/app/settings/reveal";
import { SettingsNav } from "@/app/settings/SettingsNav";
import { SettingsView } from "@/app/settings/SettingsView";
import { TooltipProvider } from "@/components/ui/tooltip";
import { accounts, overviews, previewRoutes, projects, rankings, rules, usageView } from "@/fixtures/routingData";
import type { Request } from "@/ipc/generated";
import { openSettings } from "@/state/actions";
import { useApp } from "@/state/store";

/*
 * ?drive=1 reveals one account's email, leaves the page and comes back, follows the old Usage
 * page's link, a model's rule tag and a new model's link to Routing, and searches for a row that
 * moved there, writing what it saw to
 * #providers-result (ProvidersPageRender.test.ts reads it). Without it, the page stays as it
 * opens, for screenshots (see ?reveal and ?show below).
 */
const params = new URLSearchParams(location.search);
// Driven, nothing animates or blurs (see the archived chats fixture).
if (params.has("drive")) {
  const still = document.createElement("style");
  still.textContent =
    "*, *::before, *::after { animation: none !important; transition: none !important; backdrop-filter: none !important; }";
  document.head.append(still);
}

useApp.setState((state) => ({
  info: { ...state.info, platform: "macos" } as typeof state.info,
  connection: { ...state.connection, status: "connected" } as typeof state.connection,
  settings: { ...state.settings, routingOverrides: rules, routingRankings: rankings, switchAccounts: true },
  projects,
  accounts: { accounts, warnings: [] },
  providers: { ...state.providers, view: { providers: overviews, sessions: [], fixtures: [] } },
}));

/** The accounts the daemon answers with; a step adds this computer's login again. */
let listed = accounts;

mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  switch (req.method) {
    case "getAccounts":
      return { method: req.method, accounts: { accounts: listed, warnings: [] } };
    case "updateSettings":
      return { method: req.method, settings: req.settings };
    case "getRankingsRefresh":
      return { method: req.method, refresh: null };
    case "getUsage":
      return { method: req.method, usage: usageView(req.projectId) };
    case "previewRoutes":
      return { method: req.method, routes: previewRoutes(req.areas, req.author) };
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
/** Presses a tab the way a pointer does (Radix switches tabs on mouse down). */
const press = (element: Element | undefined | null) => {
  element?.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, button: 0 }));
};
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

function Fixture() {
  const page = useApp((s) => (s.selection.type === "settings" ? s.selection.page : null));
  const [result, setResult] = useState("");
  useEffect(() => {
    openSettings("providers");
    // For screenshots: ?reveal=1 shows the second account's email, ?show=<row> brings a row
    // into view.
    if (params.has("reveal") || params.has("show")) {
      const timer = setTimeout(() => {
        if (params.has("reveal")) {
          all<HTMLButtonElement>("button").filter((element) => element.getAttribute("aria-label") === "Show email")[1]?.click();
        }
        const show = params.get("show");
        if (show) revealSetting(show);
      }, 1500);
      return () => clearTimeout(timer);
    }
    if (!params.has("drive")) return;
    const seen: Record<string, unknown> = {};
    const steps: (() => void)[] = [
      () => {
        seen.tabs = all('[data-slot="tabs-trigger"]').map(text);
        seen.status = text(document.querySelector('[data-slot="provider-panel"] > div'));
        seen.switchRow = Boolean(document.querySelector('[data-setting="Switch to another account when one runs out"]'));
        const column = document.querySelector('[data-slot="settings-page"] h1')?.closest(".mx-auto");
        const probe = document.body.appendChild(Object.assign(document.createElement("div"), { className: "max-w-settings" }));
        seen.settingsColumn = Boolean(column) && getComputedStyle(column as Element).maxWidth === getComputedStyle(probe).maxWidth;
        probe.remove();
        seen.masked = titles();
        seen.eyes = all("button").filter((element) => element.getAttribute("aria-label") === "Show email").length;
        seen.usage = all('[data-slot="account-usage"]').slice(0, 4).map((element) => {
          const share = all("span", element).find((span) => span.textContent?.endsWith("% used") && span.children.length === 0);
          return { text: text(share), tone: share?.className };
        });
        seen.models = all('[data-slot="provider-model"]').map(text);
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
        // The Usage page is gone: its links land on the accounts.
        openSettings("usage");
      },
      () => {
        seen.usageLink = {
          page: useApp.getState().selection,
          accountsInView: inView(document.querySelector('[data-setting="Accounts"]')),
        };
        all<HTMLButtonElement>("button").find((element) => text(element).startsWith("Never gets Review changes"))?.click();
      },
      () => {
        const rule = document.querySelector('[data-setting="Never use Claude Sonnet 5.5 for review"]');
        seen.ruleLink = {
          page: useApp.getState().selection,
          rule: rule?.closest('[data-slot="routing-rules"]') ? "in Rules" : "",
          inView: inView(rule),
        };
        openSettings("providers");
      },
      () => {
        press(all('[data-slot="tabs-trigger"]').find((element) => text(element).startsWith("Codex")));
      },
      () => {
        seen.codexModels = all('[data-slot="provider-model"]').map(text);
        button("Allow in Routing")?.click();
      },
      () => {
        const rule = all('[data-setting^="Don\'t give"]').find((row) => row.dataset.setting?.includes("Luna"));
        seen.allowed = { page: useApp.getState().selection, rule: rule?.dataset.setting, inView: inView(rule) };
        typeSearch("Who does what");
      },
      () => {
        seen.searched = useApp.getState().selection;
        // This computer's login is also added here (#263): one row, the added one, in use.
        const [own, ...rest] = accounts;
        if (!own?.status) return;
        const status = { ...own.status, email: "STEPHEN.GOLBAN@EXAMPLE.COM" };
        const twin = { ...own, account: { provider: own.account.provider, account: "acct-twin" }, default: true, status };
        listed = [{ ...own, default: false }, ...rest, twin];
        useApp.setState({ accounts: { accounts: listed, warnings: [] } });
        openSettings("providers");
      },
      () => {
        seen.twin = all('[data-slot="account-row"]').map((row) => ({
          title: text(row.querySelector('[data-slot="account-title"]')),
          own: text(row).includes("This computer's login"),
          inUse: text(row).includes("In use"),
          menu: Boolean(row.querySelector('button[aria-label$=": more"]')),
        }));
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
