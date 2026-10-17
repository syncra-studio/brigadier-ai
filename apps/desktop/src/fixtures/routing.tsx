/** Settings → Routing over a faked daemon: kinds of work, orders, rules and the models' ratings. */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { ROUTING_PAGE_ROWS } from "@/app/routing/RoutingPage";
import { revealSetting } from "@/app/settings/reveal";
import { SettingsNav } from "@/app/settings/SettingsNav";
import { SettingsView } from "@/app/settings/SettingsView";
import { TooltipProvider } from "@/components/ui/tooltip";
import { accounts, overviews, previewRoutes, projects, rankings, rules, usageView } from "@/fixtures/routingData";
import type { Request } from "@/ipc/generated";
import { openSettings } from "@/state/actions";
import { setModelAvailable, setProviderOn } from "@/state/providers";
import { useApp } from "@/state/store";

/*
 * ?drive=1 opens Advanced through search, opens Review changes, reads its order, the ranking, the
 * rules and overrides, asks the ranking about a change Codex wrote, removes a model from the
 * order, makes the order wait, switches to Automatic and back, opens a model's profile and adds a
 * rule, writing what it saw to #routing-result
 * (RoutingPageRender.test.ts reads it). Without it the page stays as it opens, for screenshots:
 * ?kind=<name> opens a kind, ?open=<model> opens a model's profile, ?show=<row> reveals one.
 */
const params = new URLSearchParams(location.search);
if (params.has("drive")) {
  const still = document.createElement("style");
  still.textContent =
    "*, *::before, *::after { animation: none !important; transition: none !important; backdrop-filter: none !important; }";
  document.head.append(still);
}

useApp.setState((state) => ({
  info: { ...state.info, platform: "macos" } as typeof state.info,
  connection: { ...state.connection, status: "connected" } as typeof state.connection,
  settings: { ...state.settings, routingOverrides: rules, routingRankings: rankings },
  projects,
  accounts: { accounts },
  providers: { ...state.providers, view: { providers: overviews, sessions: [], fixtures: [] } },
}));

/** Every preview asked for, in order. */
const asked: { projectId: string | null; areas: string[]; author: string | null }[] = [];

mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  switch (req.method) {
    case "getAccounts":
      return { method: req.method, accounts: { accounts } };
    case "updateSettings":
      return { method: req.method, settings: req.settings };
    case "getUsage":
      return { method: req.method, usage: usageView(req.projectId) };
    case "getRankingsRefresh":
      return { method: req.method, refresh: null };
    case "previewRoutes":
      asked.push({ projectId: req.projectId, areas: req.areas, author: req.author });
      return { method: req.method, routes: previewRoutes(req.areas, req.author) };
    default:
      return { method: req.method };
  }
});

const all = <T extends Element = HTMLElement>(selector: string, root: ParentNode = document) => [
  ...root.querySelectorAll<T>(selector),
];
const text = (element: Element | null | undefined) => element?.textContent?.trim().replaceAll(/\s+/g, " ") ?? "";
const button = (name: string, root: ParentNode = document) =>
  all<HTMLButtonElement>("button", root).find(
    (element) => text(element) === name || element.getAttribute("aria-label") === name,
  );
/** Opens a menu's trigger the way a pointer does (Radix opens menus on pointer down). */
const press = (element: Element | undefined | null) => {
  element?.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, button: 0, pointerType: "mouse" }));
};
const menuItem = (name: string) =>
  all('[role="menuitem"], [role="menuitemradio"], [role="menuitemcheckbox"]').find((item) => text(item) === name);
/** A kind of work's row in Who does what, by its name. */
const kindButton = (name: string) =>
  all<HTMLButtonElement>('[data-slot="routing-kind"] > button').find((element) => text(element).startsWith(name));
const openKind = () => document.querySelector('[data-slot="routing-kind"][data-open]');
const review = () => useApp.getState().settings.routingRankings.find((ranking) => ranking.id === "rank-review");

function Fixture() {
  const page = useApp((s) => (s.selection.type === "settings" ? s.selection.page : null));
  const [result, setResult] = useState("");
  useEffect(() => {
    openSettings("routing");
    if (!params.has("drive")) {
      const timer = setTimeout(() => {
        const kind = params.get("kind");
        if (kind) kindButton(kind)?.click();
        const open = params.get("open");
        if (open) all<HTMLButtonElement>('[data-slot="rating-row"] > button').find((row) => text(row).startsWith(open))?.click();
        const show = params.get("show");
        if (show) revealSetting(show);
      }, 800);
      return () => clearTimeout(timer);
    }
    const seen: Record<string, unknown> = {};
    const steps: (() => void)[] = [
      () => {
        // Advanced starts closed; search bringing one of its rows into view opens it.
        seen.advancedClosed = !document.querySelector('[data-slot="ranking-now"]');
        revealSetting(ROUTING_PAGE_ROWS.ratings.label);
      },
      () => {
        seen.advancedRevealed = Boolean(document.querySelector('[data-slot="ranking-now"]'));
        seen.steps = all('[data-setting]')
          .map((row) => row.dataset.setting ?? "")
          .filter((label) => /^\d\. /.test(label));
        seen.kinds = all('[data-slot="routing-kind"] > button').map(text);
        seen.openAtStart = all('[data-slot="routing-kind"][data-open]').length;
        kindButton("Review changes")?.click();
      },
      () => {
        seen.heading = text(openKind()?.querySelector("button .font-medium"));
        seen.order = text(openKind()?.querySelector('[data-slot="try-order"]'));
        seen.ranked = all('[data-slot="route-candidate"]').map(text);
        seen.blocked = all('[data-slot="blocked-candidate"]').length;
        seen.more = text(all('[data-slot="ranking-now"] button').find((element) => text(element).includes("more")));
        seen.rules = all('[data-slot="routing-rules"] [data-slot="settings-row"]').map((row) => row.dataset.setting);
        seen.ruleRows = all('[data-slot="routing-rules"] [data-slot="settings-row"]').map(text);
        seen.overrides = all('[data-slot="area-override"]').map(text);
        seen.rankingFor = text(button("The kind of work the ranking is for"));
        // Every searchable row without a replacement has a place search can bring into view.
        seen.unrevealable = Object.values(ROUTING_PAGE_ROWS)
          .filter((row) => !("target" in row))
          .map((row) => row.label)
          .filter(
            (label) =>
              !all("[data-setting]").some((element) => element.dataset.setting === label) &&
              !all("section[aria-labelledby] h2").some((heading) => text(heading) === label),
          );
        seen.authors = asked.filter((question) => question.author !== null).map((question) => question.author);
        press(button("Who wrote the change under review"));
      },
      () => {
        (menuItem("Codex") as HTMLElement | undefined)?.click();
      },
      () => {
        seen.askedCodex = asked.some((question) => question.author === "codex");
        seen.reasonAfter = text(document.querySelector('[data-slot="route-candidate"][data-chosen] p'));
        button("Remove GPT-6")?.click();
      },
      () => {
        seen.afterRemove = review()?.entries.map((entry) => (entry.target.type === "model" ? entry.target.id : ""));
        seen.orderAfterRemove = text(openKind()?.querySelector('[data-slot="try-order"] ol'));
        press(button("When none of Review changes's models can run"));
      },
      () => {
        (menuItem("wait") as HTMLElement | undefined)?.click();
      },
      () => {
        seen.waits = review()?.only;
        button("Automatic", openKind() ?? document)?.click();
      },
      () => {
        seen.afterReset = { manual: review()?.manual, kept: review()?.entries.length };
        seen.automatic = text(openKind()?.querySelector('[data-slot="try-order"]'));
        seen.reviewRow = text(kindButton("Review changes"));
        seen.ratingGroups = all('[data-slot="model-ratings"] .font-medium')
          .map(text)
          .filter((name) => ["Claude", "Codex", "Not in the registry"].includes(name));
        seen.ratingRows = all('[data-slot="rating-row"] > button').map((row) => text(row.firstElementChild));
        seen.best = all('[data-slot="rating-row"] .text-success').map(text);
        all<HTMLButtonElement>('[data-slot="rating-row"] > button').find((row) => text(row).startsWith("GPT-6"))?.click();
      },
      () => {
        const profile = document.querySelector('[data-slot="rating-row"] .grid-cols-4');
        seen.profile = all("h4", profile ?? document.body).map(text);
        seen.learned = text(profile?.querySelector(".text-2xl"));
        button("Add rule")?.click();
      },
      () => {
        seen.form = Boolean(document.querySelector('[data-slot="routing-rule-form"]'));
        button("Cancel")?.click();
      },
      () => {
        seen.formAfterCancel = Boolean(document.querySelector('[data-slot="routing-rule-form"]'));
        // The page's column, as wide as every other page's.
        const column = document.querySelector('[data-slot="settings-page"] h1')?.closest(".mx-auto");
        const probe = document.body.appendChild(Object.assign(document.createElement("div"), { className: "max-w-settings" }));
        seen.settingsColumn = Boolean(column) && getComputedStyle(column as Element).maxWidth === getComputedStyle(probe).maxWidth;
        probe.remove();
        // Scrolled so Add rule sits just under the page's top edge: a click there reaches it.
        const add = button("Add rule");
        const scroller = add?.closest(".overflow-y-auto");
        if (add && scroller) {
          scroller.scrollTop += add.getBoundingClientRect().top - scroller.getBoundingClientRect().top - 8;
        }
      },
      () => {
        const add = button("Add rule");
        const box = add?.getBoundingClientRect();
        const hit = box ? document.elementFromPoint(box.left + box.width / 2, box.top + box.height / 2) : null;
        seen.addRuleNearTop = Boolean(box && box.top < 60);
        seen.addRuleHit = Boolean(add && hit && add.contains(hit));
        // A model switched off on Providers, then its whole agent, leave the ratings.
        void setModelAvailable({ provider: "claude", id: "claude-opus-4-6" }, false);
      },
      () => {
        seen.ratingRowsModelOff = all('[data-slot="rating-row"] > button').map((row) => text(row.firstElementChild));
        void setProviderOn("codex", false);
      },
      () => {
        seen.ratingRowsAgentOff = all('[data-slot="rating-row"] > button').map((row) => text(row.firstElementChild));
        seen.bestAgentOff = all('[data-slot="rating-row"] .text-success').length;
        void setProviderOn("codex", true);
        void setModelAvailable({ provider: "claude", id: "claude-opus-4-6" }, true);
      },
      () => {
        seen.ratingRowsBack = all('[data-slot="rating-row"] > button').map((row) => text(row.firstElementChild));
        // My order brings back the order Automatic kept.
        button("My order", openKind() ?? document)?.click();
      },
      () => {
        seen.afterMyOrder = { manual: review()?.manual, kept: review()?.entries.length };
        // In a project without an order of its own, the kind follows All projects' order.
        press(button("Where these settings apply"));
      },
      () => {
        (menuItem("Brigadier Desktop") as HTMLElement | undefined)?.click();
      },
      () => {
        seen.projectMode = text(openKind()?.querySelector('[data-state="on"]'));
        seen.projectOrder = text(openKind()?.querySelector('[data-slot="try-order"]'));
        button("Automatic", openKind() ?? document)?.click();
      },
      () => {
        const own = useApp
          .getState()
          .settings.routingRankings.find((ranking) => ranking.category === "review" && ranking.projectId !== null);
        seen.projectAutomatic = own && { manual: own.manual, entries: own.entries.length };
        seen.projectModeAfter = text(openKind()?.querySelector('[data-state="on"]'));
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
      <div className="bg-background text-foreground flex h-screen">
        <aside className="w-64 shrink-0">
          <SettingsNav />
        </aside>
        <main className="flex h-full min-w-0 flex-1">
          <div className="min-w-0 flex-1">{page && <SettingsView page={page} />}</div>
        </main>
      </div>
      {result && <pre id="routing-result">{result}</pre>}
    </TooltipProvider>
  );
}

const root = document.getElementById("root");
if (root) createRoot(root).render(<Fixture />);
