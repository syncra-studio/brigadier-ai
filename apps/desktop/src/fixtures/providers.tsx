/** Settings → Providers over a faked daemon: a tab per agent, accounts with masked emails, models. */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { ProvidersHome } from "@/app/providers/ProvidersHome";
import { revealSetting } from "@/app/settings/reveal";
import { SettingsNav } from "@/app/settings/SettingsNav";
import { SettingsView } from "@/app/settings/SettingsView";
import { TooltipProvider } from "@/components/ui/tooltip";
import { accounts, overviews, previewRoutes, projects, rankings, rules, usageView } from "@/fixtures/routingData";
import type { Request } from "@/ipc/generated";
import { openSettings } from "@/state/actions";
import { useApp } from "@/state/store";
import { useUpdates } from "@/state/updates";

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

/** What Location's Change picks, in turn. */
const picks = ["/opt/elsewhere/claude", "/opt/homebrew/bin/claude"];

/** The accounts the daemon answers with; a step adds this computer's login again. */
let listed = accounts;

// The agents page shows when each limit resets: 5-hour ones in 3h, the rest in 1d 23h.
if (params.has("next")) {
  const at = Date.now();
  listed = accounts.map((view) => ({
    ...view,
    quota: view.quota && {
      ...view.quota,
      windows: view.quota.windows.map((window) => ({
        ...window,
        resetsAtMs: at + (window.windowMinutes === 300 ? 179.5 : 2850.5) * 60_000,
      })),
    },
  }));
  useApp.setState({ accounts: { accounts: listed, warnings: [] } });
}

mockIPC((command, payload) => {
  // Location's Change picks a file that isn't an agent, then one that is.
  if (command === "pick_file") return picks.shift() ?? null;
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
    case "checkAgentPath":
      if (req.path.startsWith("/opt/elsewhere/")) {
        throw { code: "invalid", message: `${req.path} isn't Claude Code: it didn't say its version` };
      }
      return { method: req.method, version: "2.1.4" };
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

/*
 * ?next=1 is the agents page drawn to the user's design (#262), not shown in the app yet; with
 * ?agent=claude it opens Claude's page. With ?drive=1 it reads the overview, opens Claude, picks a
 * binary that isn't one, goes back, and writes what it saw to #providers-next-result
 * (ProvidersHomeRender.test.ts reads it).
 */
function NextFixture() {
  const [result, setResult] = useState("");
  useEffect(() => {
    openSettings("providers");
    // A newer Claude Code is out.
    useUpdates.setState({
      view: {
        items: [
          {
            target: "claude",
            current: "2.1.0",
            latest: "2.1.4",
            action: { type: "run", command: "claude update" },
            progress: { type: "available" },
          },
        ],
      } as never,
    });
    const openAgent = () => document.querySelector<HTMLElement>(`[data-slot="agent-row"][data-setting="Claude"]`)?.click();
    if (params.get("agent") === "claude") {
      const timer = setTimeout(openAgent, 300);
      return () => clearTimeout(timer);
    }
    if (!params.has("drive")) return;
    const seen: Record<string, unknown> = {};
    const steps: (() => void)[] = [
      () => {
        seen.title = text(document.querySelector('[data-slot="settings-page"] h1'));
        seen.fallback = Boolean(document.querySelector('[data-setting="Switch to another account automatically"]'));
        seen.agents = all('[data-slot="agent-row"]').map((row) => ({
          name: row.dataset.setting,
          state: text(row.querySelector('[data-slot="agent-state"]')),
          meter: text(row.querySelector('[data-slot="account-usage"]')),
        }));
        seen.notSetUp = all('[data-slot="agent-not-set-up"]').length;
        seen.addAgent = Boolean(button("Add another agent"));
        // The row's switch doesn't open the page.
        button("Use Codex")?.click();
      },
      () => {
        seen.afterSwitch = {
          page: Boolean(document.querySelector('[data-slot="agent-page"]')),
          disabled: useApp.getState().settings.disabledProviders,
        };
        openAgent();
      },
      () => {
        seen.header = text(document.querySelector('[data-slot="settings-page"] header'));
        seen.cli = text(document.querySelector('[data-slot="agent-cli"]'));
        seen.updateEnabled = button("Update")?.disabled === false;
        seen.location = text(document.querySelector('[data-slot="agent-location"]'));
        seen.accounts = all('[data-slot="account-row"]').map((row) => ({
          title: text(row.querySelector('[data-slot="account-title"]')),
          inUse: Boolean(row.querySelector('[data-slot="in-use"]')),
          meters: all('[data-slot="account-usage"]', row).map(text),
        }));
        seen.models = all('[data-slot="provider-model"]').map(text);
        button("Change", document.querySelector('[data-slot="agent-location"]') ?? document)?.click();
      },
      () => {
        seen.refused = {
          location: text(document.querySelector('[data-slot="agent-location"]')),
          paths: useApp.getState().settings.agentPaths,
        };
        button("Change", document.querySelector('[data-slot="agent-location"]') ?? document)?.click();
      },
      () => {
        seen.chosen = {
          location: text(document.querySelector('[data-slot="agent-location"]')),
          paths: useApp.getState().settings.agentPaths,
        };
        button("Providers", document.querySelector('[data-slot="settings-page"]') ?? document)?.click();
      },
      () => {
        seen.back = Boolean(document.querySelector('[data-slot="agents-overview"]'));
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
          <div className="min-w-0 flex-1">
            <ProvidersHome />
          </div>
        </main>
      </div>
      {result && <pre id="providers-next-result">{result}</pre>}
    </TooltipProvider>
  );
}

const root = document.getElementById("root");
if (root) createRoot(root).render(params.has("next") ? <NextFixture /> : <Fixture />);
