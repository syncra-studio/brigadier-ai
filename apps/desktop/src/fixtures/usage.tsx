/**
 * Settings → Usage over a faked daemon: two Claude accounts and one Codex account, their
 * windows' share left (Limits), how they were used over the last days (Activity), and a
 * Refresh that waits until every account was checked again.
 *
 * ?drive=1 reads the cards, opens Activity and hovers its chart, switches it to 30 days,
 * presses Refresh and sends it a stale and then a fresh check, and writes what it saw to
 * #usage-result (UsagePageRender.test.ts reads it). Without it the page stays as it opens, for
 * screenshots; ?tab=activity opens Activity (&hover=1 hovers the first chart) and
 * ?refreshing=1 presses Refresh, which then waits.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { SettingsNav } from "@/app/settings/SettingsNav";
import { SettingsView } from "@/app/settings/SettingsView";
import { TooltipProvider } from "@/components/ui/tooltip";
import { account, OVERVIEWS, status, window as quotaWindow } from "@/fixtures/agents";
import type {
  AccountActivity,
  AccountRef,
  AccountUsage,
  AccountView,
  QuotaSample,
  QuotaWindow,
  Request,
  UsageView,
  WindowState,
} from "@/ipc/generated";
import { openSettings } from "@/state/actions";
import { applyEvents, useApp } from "@/state/store";
import { applyUsageEvents } from "@/state/usage";

const params = new URLSearchParams(location.search);
document.documentElement.dataset.density = params.get("density") === "compact" ? "compact" : "normal";
if (params.has("drive")) {
  const still = document.createElement("style");
  still.textContent =
    "*, *::before, *::after { animation: none !important; transition: none !important; backdrop-filter: none !important; }";
  document.head.append(still);
}

const NOW = Date.now();
const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

const OWN_CLAUDE: AccountRef = { provider: "claude" };
const WORK_CLAUDE: AccountRef = { provider: "claude", account: "acct-work" };
const OWN_CODEX: AccountRef = { provider: "codex" };

const fiveHour = (used: number, resetsInMs: number) =>
  quotaWindow("five_hour", "5-hour", used, { windowMinutes: 300, resetsAtMs: NOW + resetsInMs });
const weekly = (id: string, label: string, used: number, resetsInMs: number, fields: Partial<QuotaWindow> = {}) =>
  quotaWindow(id, label, used, { windowMinutes: 10080, resetsAtMs: NOW + resetsInMs, ...fields });

/** A window with its heat and, where given, where it is heading. */
function state(window: QuotaWindow, runsOutInMs: number | null = null): WindowState {
  const used = window.usedPercent;
  return {
    window,
    heat: used >= 100 ? "limited" : used >= 80 ? "hot" : used >= 50 ? "warm" : "cool",
    forecast:
      runsOutInMs === null
        ? null
        : { ratePerHour: 1.5, projectedAtReset: 100, runsOutAtMs: NOW + runsOutInMs, samples: 24 },
  };
}

/** Each account's windows: the own Claude login nearly out of its weekly ones. */
const WINDOWS: [AccountRef, WindowState[]][] = [
  [
    OWN_CLAUDE,
    [
      state(fiveHour(9, 3 * HOUR + 48 * MINUTE)),
      state(weekly("seven_day", "Weekly", 82, 2 * DAY + 5 * HOUR), 30 * HOUR),
      state(weekly("seven_day_fable", "Weekly (Fable)", 93, 2 * DAY + 5 * HOUR), 9 * HOUR),
    ],
  ],
  [WORK_CLAUDE, [state(fiveHour(40, 1 * HOUR + 12 * MINUTE)), state(weekly("seven_day", "Weekly", 55, 4 * DAY))]],
  [OWN_CODEX, [state(fiveHour(20, 2 * HOUR + 30 * MINUTE)), state(weekly("weekly", "Weekly", 35, 6 * DAY))]],
];

/** When each account's usage was read, in minutes before now: Refresh brings them to now. */
let readAgo: Record<string, number> = { "claude/": 4, "claude/acct-work": 12, "codex/": 2 };
const key = (ref: AccountRef) => `${ref.provider}/${ref.account ?? ""}`;

/** A provider's accounts as `getUsage` reads them. */
function usages(provider: AccountRef["provider"]): AccountUsage[] {
  return WINDOWS.filter(([ref]) => ref.provider === provider).map(([ref, windows]) => {
    const readAtMs = NOW - readAgo[key(ref)]! * MINUTE;
    return {
      account: ref,
      working: ref === OWN_CLAUDE,
      readAtMs,
      quota: { provider, windows, limit: null, heat: "hot", observedAtMs: readAtMs },
    };
  });
}

function usageView(): UsageView {
  return {
    providers: [
      {
        provider: "claude",
        accounts: usages("claude"),
        balancing: "New scouting and research go to Codex while Claude's weekly window runs hot",
      },
      { provider: "codex", accounts: usages("codex"), balancing: null },
    ],
    activity: [],
    models: [],
    learned: [],
    projectId: null,
    registry: { revision: 1, updated: "2026-10-01", source: "bundled", models: 0, fetchedAtMs: null, checkedAtMs: null, error: null } as UsageView["registry"],
  };
}

/**
 * A window's share used every two hours over 31 days: a session window climbing and resetting
 * every five hours, a weekly one climbing through each week.
 */
function samples(windowMinutes: number, scale: number, phase: number): QuotaSample[] {
  const out: QuotaSample[] = [];
  const period = windowMinutes * MINUTE;
  for (let at = NOW - 31 * DAY; at <= NOW; at += 2 * HOUR) {
    const through = ((at + phase * HOUR) % period) / period;
    out.push({ atMs: at, usedPercent: Math.min(100, Math.round(through * scale)) });
  }
  return out;
}

/** Brigadier's own tokens on an account, `scale` times a week's. */
const tokens = (scale: number) => ({
  byModel: [
    { model: "opus", tokens: 1_240_000 * scale, outputTokens: 182_000 * scale, turns: 84 * scale },
    { model: "sonnet", tokens: 310_000 * scale, outputTokens: 41_000 * scale, turns: 37 * scale },
  ],
  byConversation: [
    { conversationId: "c1", projectId: null, title: "Settings polish", tokens: 820_000 * scale },
    { conversationId: "c2", projectId: null, title: "Usage per account", tokens: 450_000 * scale },
    { conversationId: "c3", projectId: null, title: "Release notes", tokens: 280_000 * scale },
  ],
});

function activity(days: number): { sinceMs: number; accounts: AccountActivity[] } {
  const sinceMs = NOW - days * DAY;
  return {
    sinceMs,
    accounts: WINDOWS.map(([ref, windows], index) => ({
      account: ref,
      history: windows.map(({ window }, line) => ({
        windowId: window.id,
        label: window.label,
        // The Codex account was read once so far: a single reading per window.
        samples: samples(window.windowMinutes!, 70 + 15 * line - 10 * index, index * 3 + line)
          .filter((sample) => sample.atMs >= sinceMs)
          .slice(ref === OWN_CODEX ? -1 : 0),
      })),
      tokens: ref === OWN_CODEX ? { byModel: [], byConversation: [] } : tokens(days === 30 ? 4 : 1),
    })),
  };
}

let generation = 0;
const accountViews = (checkedGeneration: number): AccountView[] => [
  account("claude", undefined, "me@example.com", { status: status("claude", { email: "me@example.com" }), checkedGeneration }),
  account("claude", "acct-work", "work.person@corp.example.io", {
    status: status("claude", { email: "work.person@corp.example.io", plan: "pro" }),
    checkedGeneration,
  }),
  account("codex", undefined, "me@example.com", { status: status("codex", { email: "me@example.com", plan: "plus" }), checkedGeneration }),
];

useApp.setState((app) => ({
  info: { ...app.info, platform: "macos" } as typeof app.info,
  connection: { ...app.connection, status: "connected" } as typeof app.connection,
  settings: { ...app.settings, switchAccounts: true },
  providers: { ...app.providers, view: { providers: OVERVIEWS, sessions: [], fixtures: [] } },
  accounts: { accounts: accountViews(0) },
}));

const calls: string[] = [];

mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  switch (req.method) {
    case "getUsage":
      return { method: req.method, usage: usageView() };
    case "getUsageActivity":
      calls.push(`activity ${req.days}`);
      return { method: req.method, activity: activity(req.days) };
    case "getAccounts":
      return { method: req.method, accounts: useApp.getState().accounts };
    case "refreshAccounts":
      calls.push("refreshAccounts");
      generation += 1;
      return { method: req.method, generation };
    default:
      return { method: req.method };
  }
});

/** The daemon saying every account was checked, for the refresh `checkedGeneration`. */
function checked(checkedGeneration: number) {
  const batch = [
    {
      seq: 0,
      stream: "accounts",
      streamSeq: 0,
      atMs: Date.now(),
      event: { type: "accountsChecked", accounts: { accounts: accountViews(checkedGeneration) } },
    },
  ] as Parameters<typeof applyEvents>[0];
  applyEvents(batch);
  applyUsageEvents(batch);
}

const all = <T extends Element = HTMLElement>(selector: string, root: ParentNode = document) => [
  ...root.querySelectorAll<T>(selector),
];
const text = (element: Element | null | undefined) => element?.textContent?.trim() ?? "";
const tab = (name: string) =>
  all('[role="tab"]').find((element) => text(element) === name);
const openTab = (name: string) =>
  tab(name)?.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, button: 0 }));
const refreshButton = () => document.querySelector<HTMLButtonElement>('button[aria-label="Check every account again"]');
const spinning = () => document.querySelector<HTMLElement>('[data-slot="usage-refresh-spinner"]')?.dataset.spinning;
const checkedLines = () => all('[data-slot="usage-checked"]').map(text);
/** Hovers the first chart a third of the way in. */
function hoverChart() {
  const chart = document.querySelector<HTMLElement>('[data-slot="usage-chart"]');
  if (!chart) return;
  const box = chart.getBoundingClientRect();
  chart.dispatchEvent(
    new PointerEvent("pointermove", { bubbles: true, clientX: box.left + box.width * 0.6, clientY: box.top + box.height / 2 }),
  );
}

function Fixture() {
  const page = useApp((s) => (s.selection.type === "settings" ? s.selection.page : null));
  const [result, setResult] = useState("");
  useEffect(() => {
    openSettings("usage");
    const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
    let stopped = false;
    if (!params.has("drive")) {
      void (async () => {
        await wait(800);
        if (params.get("tab") === "activity") {
          openTab("Activity");
          await wait(800);
          if (params.has("hover")) hoverChart();
        }
        if (params.has("refreshing")) refreshButton()?.click();
      })();
      return;
    }
    const seen: Record<string, unknown> = {};
    const tooltip = () => document.querySelector('[data-slot="usage-chart-tooltip"]');
    // Each step may name what it waits for before the next reads the page.
    const steps: (() => (() => boolean) | void)[] = [
      () => {
        seen.sections = all("main section h2").map(text);
        seen.balancing = all('[data-slot="usage-balancing"]').map(text);
        seen.cards = all('[data-slot="settings-card"]').map((card) => ({
          name: text(card.querySelector('[data-slot="usage-account-name"]')),
          head: text(card.querySelector('[data-slot="usage-account"]')),
          inUse: text(card.querySelector('[data-slot="usage-account"]')).includes("In use"),
          checked: text(card.querySelector('[data-slot="usage-checked"]')),
          windows: all('[data-slot="usage-window"]', card).map((row) => ({
            level: row.dataset.level,
            title: text(row.firstElementChild),
            line: all(":scope > span:nth-child(2) > span", row).map(text),
            bar: row.querySelector<HTMLElement>('[data-slot="usage-bar"]')?.style.width,
          })),
        }));
        // The rows are static: nothing in them to press.
        seen.rowControls = all('[data-slot="usage-window"] button, [data-slot="usage-window"] [aria-expanded]').length;
        openTab("Activity");
        return () => all('[data-slot="usage-chart"] svg').length > 0 && all('[data-slot="usage-tokens"]').length > 0;
      },
      () => {
        seen.activity = {
          charts: all('[data-slot="usage-chart"]').length,
          forecasts: all('[data-slot="usage-forecasts"] li').map(text),
          tokens: all('[data-slot="usage-tokens"] > p').map(text),
          range: text(document.querySelector('[aria-label="Range"] [data-state="on"]')),
        };
        return () => {
          hoverChart();
          return tooltip() !== null;
        };
      },
      () => {
        const card = tooltip();
        seen.tooltip = card ? all(":scope > span", card).map(text) : null;
        seen.points = all('[data-slot="usage-chart-point"]').length;
        all<HTMLButtonElement>('[aria-label="Range"] button').find((button) => text(button) === "30 days")?.click();
        return () => text(document.querySelector('[data-slot="usage-tokens"] > p')).includes("30 days");
      },
      () => {
        seen.range30 = {
          range: text(document.querySelector('[aria-label="Range"] [data-state="on"]')),
          tokens: text(document.querySelector('[data-slot="usage-tokens"] > p')),
          legend: all("li", document.querySelector('[data-slot="usage-chart"]')?.previousElementSibling ?? undefined).map(text),
        };
        openTab("Limits");
        return () => all('[data-slot="usage-checked"]').length === 3;
      },
      () => {
        seen.beforeRefresh = checkedLines();
        refreshButton()?.click();
      },
      () => {
        seen.pressed = { spinning: spinning(), disabled: refreshButton()?.disabled };
        // A check that began before Refresh finishes: the page keeps waiting.
        checked(0);
      },
      () => {
        seen.stale = { spinning: spinning(), checked: checkedLines() };
        readAgo = { "claude/": 0, "claude/acct-work": 0, "codex/": 0 };
        checked(generation);
      },
      () => {
        seen.fresh = { spinning: spinning(), checked: checkedLines(), disabled: refreshButton()?.disabled };
        seen.calls = calls;
        setResult(JSON.stringify(seen));
      },
    ];
    void (async () => {
      await wait(800);
      for (const step of steps) {
        if (stopped) return;
        const ready = step();
        await wait(300);
        if (ready) for (let tries = 0; !ready() && tries < 40; tries++) await wait(100);
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
      {result && <pre id="usage-result">{result}</pre>}
    </TooltipProvider>
  );
}

const root = document.getElementById("root");
if (root) createRoot(root).render(<Fixture />);
