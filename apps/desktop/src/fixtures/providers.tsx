/** Settings → Providers over a faked daemon: the agents, and Claude's page with its accounts and models. */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { SettingsNav } from "@/app/settings/SettingsNav";
import { SettingsView } from "@/app/settings/SettingsView";
import { SidebarProvider } from "@/components/layout";
import { TooltipProvider } from "@/components/overlays";
import { fixtureInput } from "@/fixtures/input";
import { colourOf, fillsCard, surface, switchLook, wait as pause } from "@/fixtures/look";
import { accounts, overviews, projects, usageView } from "@/fixtures/providersData";
import type { AgentConnect, ProviderOverview, Request } from "@/ipc/generated";
import { openSettings } from "@/state/actions";
import { applyConnectEvents } from "@/state/connect";
import { useApp } from "@/state/store";
import { useUpdates } from "@/state/updates";

/*
 * ?drive=1 reads the agents, opens Claude's page, reveals one account's email, picks a binary
 * that isn't one and then one that is, leaves the page and comes back, follows the old Usage
 * page's link and shows this computer's login added again, writing what it saw to
 * #providers-result (ProvidersPageRender.test.ts reads it).
 * Without it the page stays as it opens, for screenshots; ?agent=claude opens Claude's page.
 * ?drive=missing starts with Codex not on this computer and walks its Set up (Connect) through
 * a failed install, Try again, its sign-in and the agent coming in, to #providers-result too;
 * with ?nodownloader=1 the install fails as it does on a Linux with neither curl nor wget, and
 * the drive opens the install guide the row links before it tries again.
 * ?drive=look (under renderFixtureLive) reads Settings › General's card and switch, then the
 * cards, an agent row under the pointer and the switches of Providers and Claude's page, to
 * #providers-result too.
 */
const params = new URLSearchParams(location.search);
// Driven, nothing animates or blurs (see the archived chats fixture).
if (params.has("drive")) {
  const still = document.createElement("style");
  still.textContent =
    "*, *::before, *::after { animation: none !important; transition: none !important; backdrop-filter: none !important; }";
  document.head.append(still);
}

const missing = params.get("drive") === "missing";
const noDownloader = params.has("nodownloader");
const look = params.get("drive") === "look";
/** Codex as the daemon finds it, in the missing drive: nowhere at first. */
const missingCodex = (fields: Partial<NonNullable<ProviderOverview["status"]>>) =>
  overviews.map((entry) =>
    entry.provider === "codex" && entry.status ? { ...entry, status: { ...entry.status, ...fields } } : entry,
  );

useApp.setState((state) => ({
  info: { ...state.info, platform: "macos" } as typeof state.info,
  connection: { ...state.connection, status: "connected" } as typeof state.connection,
  projects,
  accounts: { accounts, warnings: [] },
  providers: {
    ...state.providers,
    view: {
      providers: missing ? missingCodex({ path: null, version: null, loggedIn: false, plan: null }) : overviews,
      sessions: [],
      fixtures: [],
    },
  },
}));

/** What the faked daemon publishes as Connect's view, in the missing drive. */
const codexAt = (progress: AgentConnect["progress"] | null) =>
  applyConnectEvents([
    {
      seq: 0,
      stream: "connect",
      kind: "agent.connectChanged",
      atMs: Date.now(),
      event: { type: "agentConnectChanged", connect: { agents: progress ? [{ provider: "codex", progress }] : [] } },
    },
  ] as never);
const asked: string[] = [];

/** What Location's Change picks, in turn. */
const picks = ["/opt/elsewhere/claude", "/opt/homebrew/bin/claude"];

/** The accounts the daemon answers with; a step adds this computer's login again. */
let listed = accounts;

// Each limit resets: 5-hour ones in 3h, the rest in 1d 23h.
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

mockIPC((command, payload) => {
  // Location's Change picks a file that isn't an agent, then one that is.
  if (command === "pick_file") return picks.shift() ?? null;
  if (command === "open_url") {
    asked.push(`openUrl ${(payload as { url: string }).url}`);
    return null;
  }
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  switch (req.method) {
    case "connectAgent":
      asked.push(`connectAgent ${req.provider}`);
      codexAt({ type: "installing" });
      return { method: req.method };
    case "cancelConnect":
      asked.push(`cancelConnect ${req.provider}`);
      codexAt(null);
      return { method: req.method };
    case "getAccounts":
      return { method: req.method, accounts: { accounts: listed, warnings: [] } };
    case "updateSettings": {
      // A binary the user chose is the one the agent runs from its next check on.
      const chosen = req.settings.agentPaths.find((entry) => entry.provider === "claude")?.path;
      if (chosen) {
        const view = useApp.getState().providers.view;
        const providers = view?.providers.map((entry) =>
          entry.provider === "claude" && entry.status ? { ...entry, status: { ...entry.status, path: chosen } } : entry,
        );
        if (view && providers) useApp.setState((state) => ({ providers: { ...state.providers, view: { ...view, providers } } }));
      }
      return { method: req.method, settings: req.settings };
    }
    case "getUsage":
      return { method: req.method, usage: usageView() };
    case "checkAgentPath":
      if (req.path.startsWith("/opt/elsewhere/")) {
        throw { code: "invalid", message: `${req.path} isn't Claude Code: it didn't say its version` };
      }
      return { method: req.method, version: "2.1.4" };
    default:
      return { method: req.method };
  }
});

/** The agent page's account switch, and how it and the settings stand. */
const accountSwitch = () => document.querySelector<HTMLButtonElement>('[role="switch"][aria-label="Switch accounts automatically"]');
const accountSwitchState = () => ({
  on: accountSwitch()?.getAttribute("aria-checked") === "true",
  manual: useApp.getState().settings.manualAccounts,
});
const all = <T extends Element = HTMLElement>(selector: string, root: ParentNode = document) => [
  ...root.querySelectorAll<T>(selector),
];
const text = (element: Element | null | undefined) => element?.textContent?.trim() ?? "";
const button = (name: string, root: ParentNode = document) =>
  all<HTMLButtonElement>("button", root).find(
    (element) => text(element) === name || element.getAttribute("aria-label") === name,
  );
const pageRoot = () => document.querySelector('[data-slot="settings-page"]') ?? document;
const titles = () => all('[data-slot="account-title"]').map(text);
const locationRow = () => document.querySelector('[data-slot="agent-location"]') ?? document;
/** Every card's look, and the first switch that is on's, for the look drive. */
const cards = () => all('[data-slot="settings-card"]').map(surface);
const on = () => switchLook(document.querySelector('[data-slot="switch"][data-state="checked"]'));
const openClaude = () => document.querySelector<HTMLElement>('[data-slot="agent-row"][data-setting="Claude"]')?.click();
/** The logo before an agent page's title: whose it is, its size, and its tile's look. */
const headerMark = () => {
  const header = document.querySelector('[data-slot="settings-page"] header');
  const mark = header?.querySelector<HTMLElement>('[data-slot="agent-mark"]');
  const title = header?.querySelector("h1");
  if (!mark || !title) return null;
  const box = mark.getBoundingClientRect();
  return {
    provider: mark.dataset.provider,
    glyph: Boolean(mark.querySelector("svg")),
    size: [box.width, box.height],
    beforeTitle: box.right <= title.getBoundingClientRect().left,
    tile: surface(mark),
  };
};
/** Whether the element is in the page's scrolled view. */
const inView = (element: Element | undefined | null) => {
  if (!element) return false;
  const box = element.getBoundingClientRect();
  return box.bottom > 0 && box.top < window.innerHeight;
};
function Fixture() {
  const page = useApp((s) => (s.selection.type === "settings" ? s.selection.page : null));
  const [result, setResult] = useState("");
  useEffect(() => {
    openSettings("providers");
    if (params.get("agent") === "claude") {
      const timer = setTimeout(openClaude, 300);
      return () => clearTimeout(timer);
    }
    if (!params.has("drive")) return;
    const seen: Record<string, unknown> = {};
    if (look) {
      void (async () => {
        openSettings("general");
        await pause(500);
        seen.general = { card: surface(document.querySelector('[data-slot="settings-card"]')), switch: on() };
        openSettings("providers");
        await pause(500);
        const row = document.querySelector('[data-slot="agent-row"]');
        const box = row?.getBoundingClientRect();
        if (box) await fixtureInput({ type: "pointer", x: box.x + box.width / 3, y: box.y + box.height / 2 });
        await pause(300);
        seen.overview = {
          cards: cards(),
          switch: on(),
          hover: row && getComputedStyle(row).backgroundColor,
          wash: colourOf("bg-foreground/5"),
          fillsCard: fillsCard(row),
        };
        openClaude();
        await pause(500);
        seen.agent = { cards: cards(), switch: on() };
        setResult(JSON.stringify(seen));
      })();
      return;
    }
    const notSetUp = () =>
      all('[data-slot="agent-not-set-up"]').map((row) => ({
        text: text(row.querySelector(".flex-1")),
        status: text(row.querySelector('[data-slot="connect-status"] [role="status"]')),
        buttons: all<HTMLButtonElement>("button", row).map((element) => `${text(element)}${element.disabled ? " (off)" : ""}`),
      }));
    const steps: (() => void)[] = missing
      ? [
          () => {
            seen.agents = all('[data-slot="agent-row"]').map((row) => row.dataset.setting);
            seen.before = notSetUp();
            all<HTMLButtonElement>("button", document.querySelector('[data-slot="agent-not-set-up"]') ?? document)
              .find((element) => text(element) === "Set up")
              ?.click();
          },
          () => {
            seen.installing = notSetUp();
            codexAt(
              noDownloader
                ? {
                    type: "failed",
                    step: "install",
                    error:
                      "Installing Codex needs curl or wget, and neither is installed. Install one (for example `sudo apt install curl`) and try again, or install Codex yourself.",
                    reason: "noDownloader",
                  }
                : { type: "failed", step: "install", error: "Codex couldn't be installed. Try again." },
            );
          },
          () => {
            seen.failed = notSetUp();
            document.querySelector<HTMLButtonElement>('[data-slot="agent-not-set-up"] [data-slot="connect-guide"] button')?.click();
            button("Try again")?.click();
          },
          () => {
            // Installed and found: it moves up to the agents, its sign-in open in the browser.
            useApp.setState((state) => ({
              providers: {
                ...state.providers,
                view: { providers: missingCodex({ path: "/Users/me/.local/bin/codex", loggedIn: false, plan: null }), sessions: [], fixtures: [] },
              },
            }));
            codexAt({ type: "signingIn" });
          },
          () => {
            seen.signingIn = all('[data-slot="agent-row"]').map((row) => ({
              name: row.dataset.setting,
              state: text(row.querySelector('[data-slot="agent-state"]')),
            }));
            seen.notSetUpAfter = notSetUp().length;
            seen.terminals = document.querySelectorAll(".xterm").length;
            seen.asked = asked;
            setResult(JSON.stringify(seen));
          },
        ]
      : [
      () => {
        seen.title = text(document.querySelector('[data-slot="settings-page"] h1'));
        const column = document.querySelector('[data-slot="settings-page"] h1')?.closest(".mx-auto");
        const probe = document.body.appendChild(Object.assign(document.createElement("div"), { className: "max-w-settings" }));
        seen.settingsColumn = Boolean(column) && getComputedStyle(column as Element).maxWidth === getComputedStyle(probe).maxWidth;
        probe.remove();
        seen.agents = all('[data-slot="agent-row"]').map((row) => ({
          name: row.dataset.setting,
          state: text(row.querySelector('[data-slot="agent-state"]')),
          meter: text(row.querySelector('[data-slot="account-usage"]')),
        }));
        seen.rowMark = surface(document.querySelector('[data-slot="agent-row"] [data-slot="agent-mark"]'));
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
        openClaude();
      },
      () => {
        seen.header = text(document.querySelector('[data-slot="settings-page"] header'));
        seen.claudeMark = headerMark();
        seen.cli = text(document.querySelector('[data-slot="agent-cli"]'));
        seen.updateEnabled = button("Update")?.disabled === false;
        seen.location = text(document.querySelector('[data-slot="agent-location"]'));
        seen.accounts = all('[data-slot="account-row"]').map((row) => ({
          title: text(row.querySelector('[data-slot="account-title"]')),
          inUse: Boolean(row.querySelector('[data-slot="in-use"]')),
          meters: all('[data-slot="account-usage"]', row).map(text),
          order: text(row.querySelector('[data-slot="account-order"]')),
        }));
        seen.eyes = all("button").filter((element) => element.getAttribute("aria-label") === "Show email").length;
        seen.models = all('[data-slot="provider-model"]').map(text);
        // Reveal the second account's email only.
        all<HTMLButtonElement>("button").filter((element) => element.getAttribute("aria-label") === "Show email")[1]?.click();
        button("Change", locationRow())?.click();
      },
      () => {
        seen.revealed = titles();
        seen.refused = { location: text(document.querySelector('[data-slot="agent-location"]')), paths: useApp.getState().settings.agentPaths };
        button("Change", locationRow())?.click();
      },
      () => {
        seen.chosen = { location: text(document.querySelector('[data-slot="agent-location"]')), paths: useApp.getState().settings.agentPaths };
        openSettings("git");
      },
      () => {
        openSettings("providers");
      },
      () => {
        // Coming back shows the agents again.
        seen.cameBack = Boolean(document.querySelector('[data-slot="agents-overview"]'));
        openClaude();
      },
      () => {
        seen.afterLeaving = titles();
        // The Usage page is gone: its links land on an agent's accounts.
        openSettings("usage");
      },
      () => {
        seen.usageLink = {
          page: useApp.getState().selection,
          agent: text(document.querySelector('[data-slot="settings-page"] h1')),
          accountsInView: inView(document.querySelector('[data-setting="Accounts"]')),
        };
        // Revealed again, then back to the agents.
        all<HTMLButtonElement>("button").filter((element) => element.getAttribute("aria-label") === "Show email")[1]?.click();
        button("Providers", pageRoot())?.click();
      },
      () => {
        seen.back = Boolean(document.querySelector('[data-slot="agents-overview"]'));
        openClaude();
      },
      () => {
        seen.afterBack = titles();
        // This computer's login is also added here (#263): one row, the added one, in use.
        const [own, ...rest] = listed;
        if (!own?.status) return;
        const status = { ...own.status, email: "STEPHEN.GOLBAN@EXAMPLE.COM" };
        const twin = { ...own, account: { provider: own.account.provider, account: "acct-twin" }, default: true, inUse: true, status };
        listed = [{ ...own, default: false, inUse: false }, ...rest, twin];
        useApp.setState({ accounts: { accounts: listed, warnings: [] } });
        openSettings("accounts");
      },
      () => {
        seen.twin = all('[data-slot="account-row"]').map((row) => ({
          title: text(row.querySelector('[data-slot="account-title"]')),
          own: text(row).includes("This computer's login"),
          inUse: text(row).includes("In use"),
          menu: Boolean(row.querySelector('button[aria-label$=": more"]')),
        }));
        // Switching starts new work on the second account, not the one picked with Use: In use
        // follows the engine's pick (#432).
        const [own, second, ...others] = listed;
        if (!own || !second) return;
        useApp.setState({
          accounts: {
            accounts: [{ ...own, default: true, inUse: false }, { ...second, inUse: true }, ...others.map((view) => ({ ...view, default: false, inUse: false }))],
            warnings: [],
          },
        });
      },
      () => {
        seen.switched = all('[data-slot="account-row"]').map((row) => Boolean(row.querySelector('[data-slot="in-use"]')));
        // Claude's own account switch: on, then off, then on again.
        seen.accountSwitch = [accountSwitchState()];
        accountSwitch()?.click();
      },
      () => {
        seen.accountSwitch = [...(seen.accountSwitch as unknown[]), accountSwitchState()];
        accountSwitch()?.click();
      },
      () => {
        seen.accountSwitch = [...(seen.accountSwitch as unknown[]), accountSwitchState()];
        // Use on the third account pins it: Claude's switch turns off.
        all<HTMLButtonElement>('[data-slot="account-row"] button').filter((element) => text(element) === "Use").at(-1)?.click();
      },
      () => {
        seen.afterUse = accountSwitchState();
        // Back to the agents, then Codex's page.
        button("Providers", pageRoot())?.click();
      },
      () => {
        document.querySelector<HTMLElement>('[data-slot="agent-row"][data-setting="Codex"]')?.click();
      },
      () => {
        seen.codexMark = headerMark();
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
      {result && <pre id="providers-result">{result}</pre>}
    </TooltipProvider>
  );
}

const root = document.getElementById("root");
if (root) createRoot(root).render(<Fixture />);
