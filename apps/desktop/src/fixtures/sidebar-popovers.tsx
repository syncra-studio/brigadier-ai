/**
 * The sidebar's foot over a faked daemon: the usage list and the help menu. ?drive=1 checks the
 * foot row (Help last, flush right, no feedback icon), the list (most used first, the account in
 * use's tightest limit, amber past 75%, a reached limit said), a provider's row opening its page,
 * Providers ›, a provider that needs signing in, and the help menu's two rows, writing what it
 * saw to #popovers-result (SidebarPopoversRender.test.ts reads it), then opens a plain popover and
 * menu built from the shared parts and reads how each is drawn beside the usage list and help menu.
 * ?show=usage, ?show=limit (the account in use at its limit) or ?show=help leaves that one open,
 * for screenshots.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { AppSidebar, AppStrip } from "@/app/AppSidebar";
import { FeedbackDialog } from "@/app/feedback/FeedbackDialog";
import { SidebarFoot } from "@/app/SidebarFoot";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuTrigger,
  itemBase,
} from "@/components/overlays/dropdown-menu";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/overlays/popover";
import { SidebarPanel, SidebarProvider } from "@/components/layout/sidebar";
import { TooltipProvider } from "@/components/overlays/tooltip";
import { colourOf, surface } from "@/fixtures/look";
import type { AccountsView, AccountView, ProviderKind, ProviderOverview, QuotaWindow, Request } from "@/ipc/generated";
import { useAgentPageAsk } from "@/state/agentPage";
import { useFeedbackUi } from "@/state/feedback";
import { useApp } from "@/state/store";

const params = new URLSearchParams(location.search);
// Driven, nothing animates (see the archived chats fixture).
if (params.has("drive")) {
  const still = document.createElement("style");
  still.textContent = "*, *::before, *::after { animation: none !important; transition: none !important; }";
  document.head.append(still);
}

const HOUR = 3_600_000;
const start = Date.now();

/** A Claude account: who it is, and whether the engine starts new work on it / it was picked with Use. */
function account(id: string | undefined, email: string, inUse: boolean, picked: boolean): AccountView {
  return {
    account: { provider: "claude", ...(id ? { account: id } : {}) },
    name: "",
    default: picked,
    inUse,
    status: {
      provider: "claude",
      path: "/usr/local/bin/claude",
      version: "1.0.0",
      loggedIn: true,
      authMethod: null,
      plan: "max",
      email,
      guidance: null,
      compacts: true,
    },
    quota: null,
    quotaAtMs: start,
    checkedAtMs: start,
    checking: false,
    error: null,
    checkedGeneration: 0,
  };
}
// Picked with Use is the computer's login; new work starts on the second, whose week ends first.
const accounts: AccountsView = {
  accounts: [
    account(undefined, "golden@example.com", false, true),
    account("acct-b", "escape@example.com", true, false),
  ],
  warnings: [],
};

mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  if (req.method === "getAccounts") return { method: req.method, accounts };
  return { method: req.method };
});
const limit = (id: string, label: string, usedPercent: number, minutes: number | null, resetsInMs: number): QuotaWindow => ({
  id,
  label,
  usedPercent,
  resetsAtMs: start + resetsInMs,
  windowMinutes: minutes,
  bucket: null,
  model: null,
});

function overview(provider: ProviderKind, loggedIn: boolean, windows: QuotaWindow[]): ProviderOverview {
  return {
    provider,
    status: {
      provider,
      path: `/usr/local/bin/${provider}`,
      version: "1.0.0",
      loggedIn,
      authMethod: null,
      plan: null,
      guidance: null,
      compacts: true,
    },
    models: null,
    quota: { provider, windows, limit: null, observedAtMs: start, source: "read" },
    usage: null,
    error: null,
    checkedAtMs: start,
  };
}

const claude = overview("claude", true, [
  limit("five_hour", "5-hour", 47, 300, 2 * HOUR + 51 * 60_000),
  limit("seven_day", "Weekly", 61, 10_080, 32 * HOUR),
  limit("seven_day_fable", "Weekly (Fable)", 0, 10_080, 32 * HOUR),
]);
const limited: ProviderOverview = {
  ...claude,
  quota: {
    provider: "claude",
    windows: [
      limit("five_hour", "5-hour", 12, 300, 4 * HOUR),
      limit("seven_day", "Weekly", 100, 10_080, 3 * HOUR + 38 * 60_000),
    ],
    limit: { window: "seven_day", resetsAtMs: start + 3 * HOUR + 38 * 60_000, kind: "usageWindow" },
    observedAtMs: start,
    source: "read",
  },
};
const codex = (loggedIn: boolean) =>
  overview("codex", loggedIn, [
    limit("primary", "5-hour", 82, 300, 3 * HOUR),
    limit("secondary", "Weekly", 13, 10_080, 148 * HOUR),
  ]);
const publish = (providers: ProviderOverview[]) =>
  useApp.setState((state) => ({
    providers: { ...state.providers, view: { providers, sessions: [], fixtures: [] } },
  }));

useApp.setState({ catalogLoaded: true, connection: { status: "connected", daemon: null, reason: null } });
publish([claude, codex(true)]);

const q = <E extends Element = HTMLElement>(selector: string, root: ParentNode = document) =>
  root.querySelector<E & HTMLElement>(selector);
const all = <E extends Element = HTMLElement>(selector: string, root: ParentNode = document) =>
  [...root.querySelectorAll<E & HTMLElement>(selector)];
const text = (element: Element | null | undefined) => element?.textContent?.trim() ?? "";
const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
/** Waits for `find` to find something (up to five seconds). */
async function until<T>(find: () => T | null | undefined): Promise<T | null> {
  for (let tries = 0; tries < 100; tries++) {
    const found = find();
    if (found) return found;
    await wait(50);
  }
  return null;
}
const press = (element: Element | null | undefined) =>
  element?.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, button: 0 }));
const usageButton = () => q<HTMLButtonElement>('button[aria-label^="Usage"]');
const helpButton = () => q<HTMLButtonElement>('button[aria-label="Help"]');
/** Each row of the usage list: its words, part by part, and whether its share is amber. */
const usageRows = () =>
  all('[data-slot="usage-row"]').map((row) => ({
    words: [...row.children].map(text).filter(Boolean),
    amber: Boolean(q('[data-slot="usage-value"][data-level="warning"]', row)),
  }));
/** A row's size, corners and type, to set beside a shared menu row's. */
const rowLook = (row: Element | null | undefined) => {
  if (!row) return null;
  const css = getComputedStyle(row);
  return { height: row.getBoundingClientRect().height, radius: css.borderTopLeftRadius, font: css.fontSize };
};
/** A label's colour and type. */
const labelLook = (label: Element | null | undefined) => {
  if (!label) return null;
  const css = getComputedStyle(label);
  return { colour: css.color, font: css.fontSize, family: css.fontFamily, case: css.textTransform };
};
/** The wash a shared menu row takes when pointed at. */
const MENU_WASH = /focus:(bg-\S+)/.exec(itemBase)?.[1] ?? "";
const rowNamed = (name: string) =>
  all<HTMLButtonElement>('[data-slot="usage-row"]').find((row) => text(row).startsWith(name));

function Fixture() {
  const [result, setResult] = useState("");
  // The shared popover and menu, opened at the end of the drive to read how they're drawn.
  const [canon, setCanon] = useState(false);
  useEffect(() => {
    const show = params.get("show");
    if (!show) return;
    void (async () => {
      await until(helpButton);
      await wait(300);
      if (show === "help") {
        press(helpButton());
        return;
      }
      if (show === "limit") publish([limited, codex(true)]);
      usageButton()?.click();
    })();
  }, []);
  useEffect(() => {
    if (!params.has("drive")) return;
    let stopped = false;
    void (async () => {
      const seen: Record<string, unknown> = {};
      await until(helpButton);
      await wait(300);
      const foot = helpButton()?.closest<HTMLElement>(".flex-wrap");
      const buttons = foot ? all<HTMLButtonElement>("button[aria-label]", foot) : [];
      const help = helpButton()?.getBoundingClientRect();
      const row = foot?.getBoundingClientRect();
      const inset = foot ? Number.parseFloat(getComputedStyle(foot).paddingRight) : 0;
      seen.foot = {
        last: buttons.at(-1)?.getAttribute("aria-label"),
        flushRight: Boolean(help && row && Math.abs(row.right - inset - help.right) < 1),
        feedbackIcon: buttons.some((button) => button.getAttribute("aria-label") === "Send feedback"),
      };

      usageButton()?.click();
      await wait(200);
      seen.header = text(q('[data-slot="usage-popover"] > div'));
      seen.rows = usageRows();
      const look: Record<string, unknown> = {
        usage: surface(q('[data-slot="usage-popover"]')),
        usageRow: rowLook(q('[data-slot="usage-row"]')),
        usageLabel: labelLook(q('[data-slot="usage-popover"] > div > span')),
        usageWash: all('[data-slot="usage-row"]').every((entry) => entry.classList.contains(`hover:${MENU_WASH}`)),
        amber: getComputedStyle(q('[data-slot="usage-value"][data-level="warning"]') ?? document.body).color,
        warning: colourOf("text-warning", "color"),
      };
      seen.tips = all('[data-slot="usage-row"]').map((entry) => entry.title);
      seen.expandable = all('[data-slot="usage-row"][aria-expanded], [data-slot="usage-limit"]').length;
      rowNamed("Claude Code")?.click();
      await wait(200);
      seen.claudePage = {
        ask: useAgentPageAsk.getState().ask,
        selection: useApp.getState().selection,
        closed: !q('[data-slot="usage-popover"]'),
      };
      useAgentPageAsk.setState({ ask: null });
      usageButton()?.click();
      await wait(200);
      rowNamed("Codex")?.click();
      await wait(200);
      seen.codexPage = { ask: useAgentPageAsk.getState().ask, closed: !q('[data-slot="usage-popover"]') };
      useAgentPageAsk.setState({ ask: null });
      usageButton()?.click();
      await wait(200);
      all<HTMLButtonElement>('[data-slot="usage-popover"] button').find((button) => text(button) === "Providers")?.click();
      await wait(200);
      seen.providers = { selection: useApp.getState().selection, closed: !q('[data-slot="usage-popover"]') };

      // The account in use at its limit: the row says so, in red, and so does the icon.
      publish([limited, codex(true)]);
      usageButton()?.click();
      await wait(200);
      seen.limitRows = usageRows().map((entry) => entry.words);
      seen.limitDot = {
        tip: usageButton()?.getAttribute("aria-label"),
        red: Boolean(q('[data-slot="usage-value"][data-level="danger"]')),
      };
      usageButton()?.click();
      await wait(200);

      publish([claude, codex(false)]);
      usageButton()?.click();
      await wait(200);
      seen.signInRows = usageRows().map((entry) => entry.words);
      all<HTMLButtonElement>('[data-slot="usage-popover"] button').find((button) => text(button) === "Sign in")?.click();
      await wait(200);
      seen.signIn = { ask: useAgentPageAsk.getState().ask, closed: !q('[data-slot="usage-popover"]') };

      press(helpButton());
      await wait(200);
      seen.help = all('[role="menuitem"]').map(text);
      look.help = surface(q('[data-slot="dropdown-menu-content"]'));
      look.helpWidth = q('[data-slot="dropdown-menu-content"]')?.getBoundingClientRect().width;
      look.helpItem = rowLook(q('[role="menuitem"]'));
      all('[role="menuitem"]').find((item) => text(item) === "Keyboard shortcuts")?.click();
      await wait(200);
      seen.shortcuts = useApp.getState().selection;
      press(helpButton());
      await wait(200);
      all('[role="menuitem"]').find((item) => text(item) === "Send feedback…")?.click();
      await wait(300);
      seen.feedback = useFeedbackUi.getState().open;
      useFeedbackUi.setState({ open: false });
      await wait(200);
      setCanon(true);
      await wait(300);
      look.canon = {
        popover: surface(q('[data-slot="canon-popover"]')),
        menu: surface(q('[data-slot="canon-menu"]')),
        menuMinWidth: Number.parseFloat(getComputedStyle(q('[data-slot="canon-menu"]') as Element).minWidth),
        item: rowLook(q('[data-slot="canon-menu"] [role="menuitem"]')),
        label: labelLook(q('[data-slot="canon-menu"] [data-slot="dropdown-menu-label"]')),
      };
      seen.look = look;
      if (!stopped) setResult(JSON.stringify(seen));
    })();
    return () => {
      stopped = true;
    };
  }, []);
  return (
    <TooltipProvider>
      <div className="bg-chrome flex h-screen flex-col">
        <SidebarProvider className="relative min-h-0 flex-1 flex-col" defaultOpen>
          <div className="relative flex min-h-0 min-w-0 flex-1">
            <SidebarPanel strip={<AppStrip />} foot={<SidebarFoot />}>
              <AppSidebar />
            </SidebarPanel>
            <main className="bg-background flex-1" />
          </div>
        </SidebarProvider>
        <FeedbackDialog />
      </div>
      {canon && (
        <>
          <Popover open>
            <PopoverTrigger className="fixed top-2 left-2" />
            <PopoverContent data-slot="canon-popover">A popover</PopoverContent>
          </Popover>
          <DropdownMenu open modal={false}>
            <DropdownMenuTrigger className="fixed top-2 right-2" />
            <DropdownMenuContent data-slot="canon-menu">
              <DropdownMenuLabel>Label</DropdownMenuLabel>
              <DropdownMenuItem>An item</DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </>
      )}
      {result && <pre id="popovers-result">{result}</pre>}
    </TooltipProvider>
  );
}

const root = document.getElementById("root");
if (root) createRoot(root).render(<Fixture />);
