/** Settings → Providers over a faked daemon: the agents, and Claude's page with its accounts and models. */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { SettingsNav } from "@/app/settings/SettingsNav";
import { SettingsView } from "@/app/settings/SettingsView";
import { SidebarProvider } from "@/components/layout";
import { TooltipProvider } from "@/components/overlays";
import { account, snapshot, status as agentStatus, window as quotaWindow } from "@/fixtures/agents";
import { fixtureClick, fixtureInput } from "@/fixtures/input";
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
 * page's link and shows this computer's login added again, then every kind of account row
 * (opened and closed by pointer and keyboard, its eye, Use and ⋯ menus, this computer's login
 * renamed and cleared), writing what it saw to
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
/** Each account row as it reads: its title and line, where its eye sits, whether it opens and is open, its limits and actions. */
const rows = () =>
  all('[data-slot="account-row"]').map((row) => ({
    title: text(row.querySelector('[data-slot="account-title"]')),
    line: text(row.querySelector('[data-slot="account-line"]')),
    eye: row.querySelector('[data-slot="account-title"] ~ button[aria-label$="email"]')
      ? "title"
      : row.querySelector('[data-slot="account-line"] button[aria-label$="email"]')
        ? "line"
        : null,
    expanded: row.querySelector('[data-slot="account-toggle"]')?.getAttribute("aria-expanded") ?? null,
    usage: text(row.querySelector('[data-slot="account-header"] > span:nth-last-child(2)')),
    limits: all('[data-slot="account-limit"]', row).map(text),
    actions: all<HTMLButtonElement>('[data-slot="account-header"] > div:last-child button', row).map((element) => text(element) || element.getAttribute("aria-label")),
    inUse: Boolean(row.querySelector('[data-slot="in-use"]')),
  }));
const openRows = () => all('[data-slot="account-row"]').map((row) => row.hasAttribute("data-open"));
const accountRow = (index: number) => all('[data-slot="account-row"]')[index];
/** The ⋯ menu of row `index` opened by a real click: its items, then closed with Escape. */
const menuOf = async (index: number) => {
  const more = accountRow(index)?.querySelector('button[aria-label$=": more"]');
  if (more) await fixtureClick(more);
  await pause(200);
  return all('[role="menuitem"]').map(text);
};
const escape = async () => {
  await fixtureInput({ type: "key", key: "Escape" });
  await pause(150);
};
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
/**
 * This computer's login renamed from its ⋯ menu with `keys`: the field's first value, which rows
 * are open, the names saved and whether Settings stays open.
 */
const rename = async (keys: () => Promise<void>) => {
  await menuOf(0);
  const item = all('[role="menuitem"]').find((element) => text(element) === "Rename");
  if (item) await fixtureClick(item);
  await pause(200);
  const field = accountRow(0)?.querySelector<HTMLInputElement>('input[aria-label="Account name"]');
  if (field) await fixtureClick(field);
  await pause(100);
  const shown = { field: field?.value ?? null, open: openRows() };
  await keys();
  await pause(250);
  return {
    ...shown,
    names: useApp.getState().settings.ownAccountNames,
    stillOpen: Boolean(document.querySelector('[data-slot="agent-page"]')),
  };
};
/** `key` pressed `times` times. */
const pressKey = (key: string, times = 1) => async () => {
  for (let i = 0; i < times; i++) await fixtureInput({ type: "key", key });
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
    const steps: (() => void | Promise<void>)[] = missing
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
        // One text node, so a screen reader reads "Max" whole, not "M", "ax" (#580).
        seen.planChipNodes = document.querySelector('[data-slot="plan-chip"]')?.childNodes.length;
        // The meter's name isn't cut off ("Weekly · in u…") beside the percent (#579).
        const meterName = document.querySelector('[data-slot="agent-row"][data-setting="Claude"] [data-slot="account-usage"] span.truncate');
        seen.meterOverflow = meterName ? meterName.scrollWidth > meterName.clientWidth : null;
        const claudeRow = document.querySelector('[data-slot="agent-row"][data-setting="Claude"]');
        seen.rowOverflow = claudeRow ? claudeRow.scrollWidth > claudeRow.clientWidth : null;
        seen.notSetUp = all('[data-slot="agent-not-set-up"]').length;
        seen.addAgent = Boolean(button("Add another agent"));
        // The fallback switch (S38): off by default, and its click turns it on.
        const fallback = document.querySelector<HTMLButtonElement>(
          '[role="switch"][aria-label="Switch to another agent automatically"]',
        );
        seen.fallback = {
          row: Boolean(document.querySelector('[data-setting="Switch to another agent automatically"]')),
          on: fallback?.getAttribute("aria-checked") === "true",
        };
        fallback?.click();
        // The row's switch doesn't open the page.
        button("Use Codex")?.click();
      },
      () => {
        seen.fallbackAfter = useApp.getState().settings.switchProviders;
        seen.afterSwitch = {
          page: Boolean(document.querySelector('[data-slot="agent-page"]')),
          disabled: useApp.getState().settings.disabledProviders,
        };
        // Opened as a screen reader presses the row: a click alone on its button, a real one
        // with no control inside it to take the press, and no switch inside a role=button (#564).
        const press = document.querySelector<HTMLElement>('[data-slot="agent-row"][data-setting="Claude"] [data-slot="agent-row-open"]');
        seen.rowPress = {
          tag: press?.tagName,
          label: press?.getAttribute("aria-label"),
          inside: press?.children.length,
          switchInButton: Boolean(document.querySelector('[role="button"] [role="switch"]')),
        };
        press?.click();
      },
      () => {
        seen.disabledAfterPress = useApp.getState().settings.disabledProviders;
        seen.header = text(document.querySelector('[data-slot="settings-page"] header'));
        seen.claudeMark = headerMark();
        seen.cli = text(document.querySelector('[data-slot="agent-cli"]'));
        seen.updateEnabled = button("Update")?.disabled === false;
        seen.location = text(document.querySelector('[data-slot="agent-location"]'));
        seen.rows = rows();
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
        // Every kind of row (D2, D7, D11, D12, D14): this computer's login named, an account
        // named by its email at 93%, one whose usage isn't read yet, and one signed out.
        const hour = 3_600_000;
        const limits = snapshot("claude", [
          quotaWindow("seven_day", "Weekly", 41, { windowMinutes: 10_080, resetsAtMs: Date.now() + 50 * hour }),
          quotaWindow("five_hour", "5-hour", 24, { windowMinutes: 300, resetsAtMs: Date.now() + 2 * hour + 15 * 60_000 }),
          quotaWindow("seven_day_fable", "Weekly (Fable)", 12, { windowMinutes: 10_080, resetsAtMs: Date.now() + 50 * hour }),
        ]);
        listed = [
          account("claude", undefined, "stephen.golban@example.com", { name: "Personal", quota: limits, quotaAtMs: Date.now(), inUse: true }),
          account("claude", "acct-work", "work.person@corp.example.io", {
            name: "work.person@corp.example.io",
            quota: snapshot("claude", [quotaWindow("five_hour", "5-hour", 61, { windowMinutes: 300 }), quotaWindow("seven_day", "Weekly", 93, { windowMinutes: 10_080 })]),
            quotaAtMs: Date.now(),
            inUse: false,
          }),
          account("claude", "acct-side", "side@example.org", { name: "Side project", inUse: false }),
          account("claude", "acct-out", "m@proton.me", { status: agentStatus("claude", { email: "m@proton.me", loggedIn: false, plan: null }), inUse: false }),
        ];
        useApp.setState({ accounts: { accounts: listed, warnings: [] } });
      },
      async () => {
        seen.kinds = rows();
        seen.kindsOpen = openRows();
        // The eye shows the email and doesn't toggle the row.
        const eye = accountRow(0)?.querySelector('[data-slot="account-line"] button');
        if (eye) await fixtureClick(eye);
        await pause(150);
        seen.afterEye = { open: openRows(), line: text(accountRow(0)?.querySelector('[data-slot="account-line"]')) };
        // A press on a closed row's name opens it; the first stays open.
        const title = accountRow(1)?.querySelector('[data-slot="account-title"]');
        if (title) await fixtureClick(title);
        await pause(150);
        seen.twoOpen = { open: openRows(), expanded: rows().map((row) => row.expanded), limits: rows()[1]?.limits };
        // The keyboard: Enter on the row's toggle closes it, Space opens it again.
        accountRow(0)?.querySelector<HTMLButtonElement>('[data-slot="account-toggle"]')?.focus();
        seen.toggleFocused = document.activeElement?.getAttribute("data-slot");
        seen.toggleNested = all('[data-slot="account-toggle"] button, [data-slot="account-toggle"] input').length;
        await fixtureInput({ type: "key", key: "Enter" });
        await pause(150);
        seen.afterEnter = openRows();
        await fixtureInput({ type: "key", key: " " });
        await pause(150);
        seen.afterSpace = openRows();
        // Use picks the open second account and leaves it open.
        const use = all<HTMLButtonElement>("button", accountRow(1)).find((element) => text(element) === "Use");
        if (use) await fixtureClick(use);
        await pause(200);
        seen.afterUseClick = { open: openRows(), manual: useApp.getState().settings.manualAccounts };
        // Each row's ⋯ menu, none of them toggling it.
        seen.menus = [];
        for (const index of [0, 1, 3]) {
          (seen.menus as unknown[]).push(await menuOf(index));
          await escape();
        }
        seen.afterMenus = openRows();
      },
      async () => {
        // Renaming this computer's login, then clearing it back to its email, then a cancelled edit.
        seen.renamed = await rename(async () => {
          await pressKey("Backspace", "Personal".length)();
          await fixtureInput({ type: "type", text: "Home" });
          await pressKey("Enter")();
        });
        seen.cleared = await rename(async () => {
          await pressKey("Backspace", "Personal".length)();
          await pressKey("Enter")();
        });
        seen.cancelled = await rename(async () => {
          await fixtureInput({ type: "type", text: "X" });
          await pressKey("Escape")();
        });
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
        await step();
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
