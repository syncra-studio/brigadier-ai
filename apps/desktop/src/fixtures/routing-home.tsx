/**
 * The new Settings → Routing (#278, drawn to the user's design, not shown in the app yet) over a
 * faked daemon: kinds of work, orders, rules, model strength and how a model is picked.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { ROUTING_HOME_ROWS, RoutingHome } from "@/app/routing/RoutingHome";
import { revealSetting } from "@/app/settings/reveal";
import { SettingsNav } from "@/app/settings/SettingsNav";
import { Toaster } from "@/components/ui/toast";
import { TooltipProvider } from "@/components/ui/tooltip";
import { accounts, overviews, previewRoutes, projects, rankings, rules, usageView } from "@/fixtures/routingData";
import type { RankingsRefresh, Request, RoutePreview } from "@/ipc/generated";
import { openSettings } from "@/state/actions";
import { setModelAvailable, setProviderOn } from "@/state/providers";
import { useApp } from "@/state/store";
import { useToasts } from "@/state/toasts";

/*
 * ?drive=1 reads who does what, opens Review changes, removes a model from its order, switches it
 * to Automatic and back, follows See strengths, writes a rule (its preview, a clash, Add, Undo),
 * opens How a model is picked through search and switches models off and on, writing what it saw
 * to #routing-result (RoutingPageRender.test.ts reads it). Without it the page stays as it opens,
 * for screenshots: ?auto=<category> sets a kind to Automatic first, ?kind=<name> opens a kind,
 * ?more=strength|steps opens a row of More, ?rule=1|2|2b|3 shows a rule being added (a draft, its
 * preview, a clash, added). ?drive=1&edge=1 instead opens Research while it waits with no order
 * of its own and asks for My order, then reads a ratings refresh that failed.
 */
const params = new URLSearchParams(location.search);
if (params.has("drive")) {
  const still = document.createElement("style");
  still.textContent =
    "*, *::before, *::after { animation: none !important; transition: none !important; backdrop-filter: none !important; }";
  document.head.append(still);
}

const auto = params.get("auto");
const edge = params.has("edge");
useApp.setState((state) => ({
  info: { ...state.info, platform: "macos" } as typeof state.info,
  connection: { ...state.connection, status: "connected" } as typeof state.connection,
  settings: {
    ...state.settings,
    routingOverrides: rules,
    routingRankings: rankings
      .filter((ranking) => !(edge && ranking.category === "research"))
      .map((ranking) => (ranking.category === auto ? { ...ranking, manual: false } : ranking)),
  },
  projects,
  accounts: { accounts, warnings: [] },
  providers: { ...state.providers, view: { providers: overviews, sessions: [], fixtures: [] } },
}));

/** ?edge: a refresh that failed after it started, and a kind with nothing that can run. */
const FAILED_REFRESH: RankingsRefresh = {
  state: "failed",
  jobId: "job",
  startedAtMs: Date.now() - 60_000,
  finishedAtMs: Date.now() - 30_000,
  model: null,
  provider: null,
  baseRevision: null,
  changes: [],
  errors: ["the research model timed out"],
  overlayApplied: true,
  overlayAtMs: null,
};
const WAITING: RoutePreview["outcome"] = { type: "wait", reason: "", resetsAtMs: null, rule: null, ranking: null };

/** Every preview asked for, in order. */
const asked: { projectId: string | null; areas: string[]; author: string | null }[] = [];

mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  switch (req.method) {
    case "getAccounts":
      return { method: req.method, accounts: { accounts, warnings: [] } };
    case "updateSettings":
      return { method: req.method, settings: req.settings };
    case "getUsage":
      return { method: req.method, usage: usageView(req.projectId) };
    case "getRankingsRefresh":
      return { method: req.method, refresh: edge ? FAILED_REFRESH : null };
    case "previewRoutes": {
      asked.push({ projectId: req.projectId, areas: req.areas, author: req.author });
      const routes = previewRoutes(req.areas, req.author);
      return {
        method: req.method,
        routes: edge
          ? routes.map((route) => (route.category === "research" ? { ...route, outcome: WAITING } : route))
          : routes,
      };
    }
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
  all<HTMLElement>('[role="menuitem"], [role="menuitemradio"], [role="menuitemcheckbox"]').find((item) =>
    text(item).startsWith(name),
  );
/** A kind of work's row in Who does what, by its name. */
const kindButton = (name: string) =>
  all<HTMLButtonElement>('[data-slot="routing-kind"] > button').find((element) => text(element).startsWith(name));
const openKind = () => document.querySelector('[data-slot="routing-kind"][data-open]');
const strengthNames = () => all('[data-slot="strength-name"]').map(text);
const review = () => useApp.getState().settings.routingRankings.find((ranking) => ranking.id === "rank-review");
const ruleRows = () => all('[data-slot="routing-rule"]').map(text);
const form = () => document.querySelector('[data-slot="routing-rule-form"]');
const note = () => text(form()?.querySelector('[role="status"], [role="alert"]'));
/** Picks `item` from the menu `trigger` opens: one step presses, the next picks. */
const pick = (trigger: string, item: string): (() => void)[] => [
  () => press(button(trigger)),
  () => menuItem(item)?.click(),
];

/** The steps that show a rule being added, up to `stage`, for screenshots. */
function ruleSteps(stage: string): (() => void)[] {
  const steps: (() => void)[] = [() => (document.querySelector('[data-slot="add-rule"]') as HTMLButtonElement | null)?.click()];
  if (stage === "1") return steps;
  steps.push(...pick("What the rule does", "Prefer"));
  steps.push(...pick("The model the rule is about", stage === "2b" ? "Sonnet 5.5" : "GPT-6"));
  steps.push(...pick("The kind of work the rule is about", stage === "2b" ? "Review changes" : "Write commit messages"));
  if (stage === "3") steps.push(() => button("Add rule", form() ?? document)?.click());
  return steps;
}

function Fixture() {
  const page = useApp((s) => (s.selection.type === "settings" ? s.selection.page : null));
  const [result, setResult] = useState("");
  useEffect(() => {
    openSettings("routing");
    const seen: Record<string, unknown> = {};
    let steps: (() => void)[];
    if (!params.has("drive")) {
      steps = [
        () => {
          const kind = params.get("kind");
          if (kind) kindButton(kind)?.click();
          const more = params.get("more");
          if (more === "strength") revealSetting(ROUTING_HOME_ROWS.strength.label);
          if (more === "steps") revealSetting(ROUTING_HOME_ROWS.steps.label);
        },
        ...(params.has("rule") ? ruleSteps(params.get("rule") ?? "") : []),
      ];
    } else if (edge) {
      steps = [
        () => kindButton("Research")?.click(),
        () => button("My order", openKind() ?? document)?.click(),
        () => {
          seen.mode = text(openKind()?.querySelector('[data-state="on"]'));
          seen.addModel = Boolean(button("Add model", openKind() ?? document));
          revealSetting(ROUTING_HOME_ROWS.strength.label);
        },
        () => {
          seen.failure = text(document.querySelector('[data-slot="model-strength"] [role="alert"]'));
          setResult(JSON.stringify(seen));
        },
      ];
    } else {
      steps = [
        () => {
          seen.note = text(document.querySelector('[data-slot="settings-page"] header'));
          seen.sections = all("section h2").map(text);
          seen.kinds = all('[data-slot="routing-kind"] > button').map(text);
          seen.openAtStart = all('[data-slot="routing-kind"][data-open]').length;
          kindButton("Review changes")?.click();
        },
        () => {
          seen.mode = text(openKind()?.querySelector('[data-state="on"]'));
          seen.whoPicks = text(openKind()?.querySelector('[data-slot="who-picks"] > div'));
          seen.order = all('[data-slot="ranked-place"]', openKind() ?? document).map(text);
          press(button("GPT-6: actions"));
        },
        () => menuItem("Remove")?.click(),
        () => {
          seen.afterRemove = review()?.entries.map((entry) => (entry.target.type === "model" ? entry.target.id : ""));
          button("Automatic", openKind() ?? document)?.click();
        },
        () => {
          seen.afterReset = { manual: review()?.manual, kept: review()?.entries.length };
          seen.reviewRow = text(kindButton("Review changes"));
          seen.picks = all('[data-slot="auto-pick"]').map(text);
          seen.skipped = text(openKind()?.querySelector('[data-slot="auto-picks"] p'));
          button("See strengths")?.click();
        },
        () => {
          seen.strengthFor = text(button("The kind of work the models are ranked for"));
          seen.strength = all('[data-slot="strength-row"]').map(text);
          seen.ratingsFooter = text(document.querySelector('[data-slot="model-strength"] > div:last-of-type'));
          button("My order", openKind() ?? document)?.click();
        },
        () => {
          seen.afterMyOrder = { manual: review()?.manual, kept: review()?.entries.length };
          seen.rules = ruleRows();
          (document.querySelector('[data-slot="add-rule"]') as HTMLButtonElement | null)?.click();
        },
        () => {
          seen.form = text(form());
          seen.addDisabled = button("Add rule", form() ?? document)?.disabled;
          seen.headerAddDisabled = (document.querySelector('[data-slot="add-rule"]') as HTMLButtonElement | null)?.disabled;
        },
        ...pick("What the rule does", "Prefer"),
        ...pick("The model the rule is about", "Sonnet 5.5"),
        ...pick("The kind of work the rule is about", "Review changes"),
        () => {
          seen.clash = note();
        },
        ...pick("The model the rule is about", "GPT-6"),
        ...pick("The kind of work the rule is about", "Write commit messages"),
        () => {
          seen.preview = note();
          button("Add rule", form() ?? document)?.click();
        },
        () => {
          seen.formAfterAdd = Boolean(form());
          seen.rulesAfterAdd = ruleRows();
          const added = useToasts.getState().toasts.at(-1);
          seen.toast = added && [added.text, ...added.actions.map((action) => action.label)];
          added?.actions[0]?.run();
        },
        () => {
          seen.rulesAfterUndo = ruleRows();
          revealSetting(ROUTING_HOME_ROWS.steps.label);
        },
        () => {
          seen.steps = all('[data-slot="routing-steps"] li').map(text);
          seen.strengthClosed = !document.querySelector('[data-slot="model-strength"]');
          // Every searchable row has a place search can bring into view.
          revealSetting(ROUTING_HOME_ROWS.strength.label);
        },
        () => {
          seen.unrevealable = Object.values(ROUTING_HOME_ROWS)
            .map((row) => ("target" in row ? row.target : row.label))
            .filter(
              (label) =>
                !all("[data-setting]").some((element) => element.dataset.setting === label) &&
                !all("section[aria-labelledby] h2").some((heading) => text(heading) === label),
            );
          // The page's column, as wide as every other page's.
          const column = document.querySelector('[data-slot="settings-page"] h1')?.closest(".mx-auto");
          const probe = document.body.appendChild(Object.assign(document.createElement("div"), { className: "max-w-settings" }));
          seen.settingsColumn = Boolean(column) && getComputedStyle(column as Element).maxWidth === getComputedStyle(probe).maxWidth;
          probe.remove();
          // Scrolled so Add rule sits just under the page's top edge: a click there reaches it.
          const add = document.querySelector<HTMLElement>('[data-slot="add-rule"]');
          const scroller = add?.closest(".overflow-y-auto");
          // Room below the page, so it can scroll that far.
          scroller?.appendChild(Object.assign(document.createElement("div"), { className: "h-screen shrink-0" }));
          if (add && scroller) scroller.scrollTop += add.getBoundingClientRect().top - scroller.getBoundingClientRect().top - 8;
        },
        () => {
          const add = document.querySelector<HTMLElement>('[data-slot="add-rule"]');
          const box = add?.getBoundingClientRect();
          const hit = box ? document.elementFromPoint(box.left + box.width / 2, box.top + box.height / 2) : null;
          seen.addRuleNearTop = Boolean(box && box.top < 60);
          seen.addRuleHit = Boolean(add && hit && add.contains(hit));
          // A model switched off on Providers, then its whole agent, leave Model strength.
          void setModelAvailable({ provider: "claude", id: "claude-opus-4-6" }, false);
        },
        () => {
          seen.strengthModelOff = strengthNames();
          void setProviderOn("codex", false);
        },
        () => {
          seen.strengthAgentOff = strengthNames();
          void setProviderOn("codex", true);
          void setModelAvailable({ provider: "claude", id: "claude-opus-4-6" }, true);
        },
        () => {
          seen.strengthBack = strengthNames();
          setResult(JSON.stringify(seen));
        },
      ];
    }
    let stopped = false;
    const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
    void (async () => {
      await wait(params.has("drive") ? 600 : 800);
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
          <div className="min-w-0 flex-1">{page && <RoutingHome />}</div>
        </main>
      </div>
      <Toaster className="top-2" />
      {result && <pre id="routing-result">{result}</pre>}
    </TooltipProvider>
  );
}

const root = document.getElementById("root");
if (root) createRoot(root).render(<Fixture />);
