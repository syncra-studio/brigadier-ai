/**
 * Settings → Routing over a faked daemon and a faked rules file: kinds of work and your rows,
 * rules, model strength and how a model is picked.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { GENERAL_ROWS } from "@/app/settings/generalRows";
import { ROUTING_PAGE_ROWS } from "@/app/settings/routing/RoutingPage";
import { revealSetting } from "@/app/settings/reveal";
import { SettingsNav } from "@/app/settings/SettingsNav";
import { SettingsView } from "@/app/settings/SettingsView";
import { Toaster, TooltipProvider } from "@/components/overlays";
import { SidebarProvider } from "@/components/layout";
import { surface } from "@/fixtures/look";
import { accounts, overviews, projects, usageView } from "@/fixtures/providersData";
import type { GuidanceRow, ModelRule, ModelRules, Request } from "@/ipc/generated";
import { openSettings } from "@/state/actions";
import { useApp } from "@/state/store";
import { useToasts } from "@/state/toasts";

/*
 * ?drive=1 reads who does what and the rules from the fixture's rules file, opens a shipped kind
 * and makes it yours, changes its effort and removes it, adds a rule (its preview, a clash, Add,
 * Undo), edits the file by hand and opens the page again, then opens How a model is picked
 * through search and presses Refresh models, writing what it saw (and what the page wrote to the file) to #routing-result
 * (RoutingPageRender.test.ts reads it). Without it the page stays as it opens, for screenshots:
 * ?kind=<name> opens a kind, ?more=strength|steps opens a row of More, ?rule=1|2|3 shows a rule
 * being added (a draft, its preview, added). ?drive=look reads Settings › General's card, select
 * and chosen pill, then Routing's cards, a select and the chosen pill of an opened kind, to
 * #routing-result too.
 */
const params = new URLSearchParams(location.search);
if (params.has("drive")) {
  const still = document.createElement("style");
  still.textContent =
    "*, *::before, *::after { animation: none !important; transition: none !important; backdrop-filter: none !important; }";
  document.head.append(still);
}

const look = params.get("drive") === "look";
/** A card's, a select's and a chosen pill's look, under `root`; the pill in `choices` if given. */
const looks = (root: ParentNode, cards: string, choices = "") => ({
  cards: all(cards, root).map(surface),
  select: surface(root.querySelector('[data-slot="settings-page"] button[aria-haspopup="menu"]')),
  chosen: surface(root.querySelector(`[data-slot="settings-page"] ${choices} button[role="radio"][data-state="on"]`)),
});
useApp.setState((state) => ({
  info: { ...state.info, platform: "macos" } as typeof state.info,
  connection: { ...state.connection, status: "connected" } as typeof state.connection,
  projects,
  accounts: { accounts, warnings: [] },
  providers: { ...state.providers, view: { providers: overviews, sessions: [], fixtures: [] } },
}));

/** The shipped guidance, as the daemon reads it. */
const SHIPPED: GuidanceRow[] = [
  { work: "Architecture and broad changes", agent: "claude", model: "opus", effort: "high", reason: "Strong default for work that needs judgment across a codebase." },
  { work: "Bounded implementation", agent: "codex", model: "default", effort: "high", reason: "The CLI default suits a well specified coding task." },
  { work: "Mechanical changes and search", agent: "claude", model: "sonnet", effort: "medium", reason: "A smaller model fits a bounded task with clear checks." },
  { work: "Independent review", agent: "codex", model: "default", effort: "high", reason: "A second provider offers a useful independent reading." },
];

/** The fixture's rules file: the user's rows (one over a shipped row, one of their own) and rules. */
const file: { rows: GuidanceRow[]; rules: ModelRule[] } = {
  rows: [
    { work: "Bounded implementation", agent: "claude", model: "sonnet", effort: "medium", reason: "My preference." },
    { work: "Release notes", agent: "codex", model: "gpt-6", effort: "low", reason: "Short and cheap." },
  ],
  rules: [
    { type: "forbid", names: ["fable"] },
    { type: "effortCeiling", effort: "high" },
  ],
};
const read = (): ModelRules => ({ path: "/Users/me/.brigadier/models.md", shipped: SHIPPED, rows: [...file.rows], rules: [...file.rules] });
/** Every request's method, in order. */
const asked: string[] = [];
/** Every save the page sent, in order. */
const writes: { rows: GuidanceRow[]; rules: ModelRule[] }[] = [];
let reads = 0;

mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  asked.push(req.method);
  switch (req.method) {
    case "getAccounts":
      return { method: req.method, accounts: { accounts, warnings: [] } };
    case "updateSettings":
      return { method: req.method, settings: req.settings };
    case "getUsage":
      return { method: req.method, usage: usageView() };
    case "refreshAccounts":
      return { method: req.method, generation: 0 };
    case "checkRegistry":
      return { method: req.method, registry: usageView().registry };
    case "getModelRules":
      reads += 1;
      return { method: req.method, rules: read() };
    case "setModelRules":
      writes.push({ rows: req.rows, rules: req.rules });
      file.rows = req.rows;
      file.rules = req.rules;
      return { method: req.method, rules: read() };
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
const kinds = () => all('[data-slot="routing-kind"] > button').map(text);
const ruleRows = () => all('[data-slot="routing-rule"]').map(text);
const form = () => document.querySelector('[data-slot="routing-rule-form"]');
const note = () => text(form()?.querySelector('[role="status"], [role="alert"]'));
const lastWrite = () => writes.at(-1);
/** Picks `item` from the menu `trigger` opens: one step presses, the next picks. */
const pick = (trigger: string, item: string): (() => void)[] => [
  () => press(button(trigger)),
  () => menuItem(item)?.click(),
];

/** The steps that show a rule being added, up to `stage`, for screenshots. */
function ruleSteps(stage: string): (() => void)[] {
  const steps: (() => void)[] = [() => (document.querySelector('[data-slot="add-rule"]') as HTMLButtonElement | null)?.click()];
  if (stage === "1") return steps;
  steps.push(...pick("The model the rule is about", "GPT-6"));
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
          if (more === "strength") revealSetting(ROUTING_PAGE_ROWS.strength.label);
          if (more === "steps") revealSetting(ROUTING_PAGE_ROWS.steps.label);
        },
        ...(params.has("rule") ? ruleSteps(params.get("rule") ?? "") : []),
      ];
    } else if (look) {
      steps = [
        () => openSettings("general"),
        () => {
          // General's pills are Follow-up's; its sidebar choice is two pictures, not pills.
          seen.general = looks(document, '[data-slot="settings-card"]', `[aria-label="${GENERAL_ROWS.followUp.label}"]`);
          openSettings("routing");
        },
        () => kindButton("Bounded implementation")?.click(),
        () => {
          seen.routing = looks(document, '[data-slot="who-does-what"], [data-slot="routing-rules"], [data-slot="routing-more"]');
          setResult(JSON.stringify(seen));
        },
      ];
    } else {
      steps = [
        () => {
          seen.note = text(document.querySelector('[data-slot="settings-page"] header'));
          seen.sections = all("section h2").map(text);
          seen.kinds = kinds();
          seen.rules = ruleRows();
          seen.openAtStart = all('[data-slot="routing-kind"][data-open]').length;
          seen.readsAtOpen = reads;
          kindButton("Architecture and broad changes")?.click();
        },
        () => {
          seen.mode = text(openKind()?.querySelector('[data-state="on"]'));
          seen.whoPicks = text(openKind()?.querySelector('[data-slot="who-picks"] > div'));
          seen.shipped = text(openKind()?.querySelector('[data-slot="auto-picks"]'));
          button("Mine", openKind() ?? document)?.click();
        },
        () => {
          seen.afterMine = lastWrite()?.rows;
          seen.mine = all('[data-slot="ranked-place"]', openKind() ?? document).map(text);
          press(button("Opus 5.5: effort"));
        },
        () => {
          // The effort menu stops at the user's ceiling.
          seen.efforts = all('[role="menuitemradio"]').map(text);
          menuItem("Medium")?.click();
        },
        () => {
          seen.afterEffort = lastWrite()?.rows.find((row) => row.work === "Architecture and broad changes");
          seen.kindLine = text(kindButton("Architecture and broad changes"));
          press(button("Opus 5.5: actions"));
        },
        () => menuItem("Remove")?.click(),
        () => {
          seen.afterRemove = lastWrite()?.rows.map((row) => row.work);
          seen.modeAfterRemove = text(openKind()?.querySelector('[data-state="on"]'));
          (document.querySelector('[data-slot="add-rule"]') as HTMLButtonElement | null)?.click();
        },
        () => {
          seen.form = text(form());
          seen.addDisabled = button("Add rule", form() ?? document)?.disabled;
          seen.headerAddDisabled = (document.querySelector('[data-slot="add-rule"]') as HTMLButtonElement | null)?.disabled;
        },
        ...pick("The model the rule is about", "Opus 5.5"),
        () => {
          seen.clash = note();
        },
        ...pick("The model the rule is about", "GPT-6"),
        () => {
          seen.preview = note();
          button("Add rule", form() ?? document)?.click();
        },
        () => {
          seen.formAfterAdd = Boolean(form());
          seen.writtenRules = lastWrite()?.rules;
          seen.rulesAfterAdd = ruleRows();
          const added = useToasts.getState().toasts.at(-1);
          seen.toast = added && [added.text, ...added.actions.map((action) => action.label)];
          added?.actions[0]?.run();
        },
        () => {
          seen.rulesAfterUndo = ruleRows();
          // A hand edit of the file while the page is away, then the page opened again.
          file.rows = [...file.rows, { work: "Docs", agent: "claude", model: "sonnet", effort: "low", reason: "Hand edit." }];
          file.rules = [...file.rules, { type: "allow", agent: "codex", names: ["gpt-6"] }];
          openSettings("general");
        },
        () => openSettings("routing"),
        () => {
          seen.kindsAfterHandEdit = kinds();
          seen.rulesAfterHandEdit = ruleRows();
          revealSetting(ROUTING_PAGE_ROWS.steps.label);
        },
        () => {
          seen.steps = all('[data-slot="routing-steps"] li').map(text);
          seen.strengthClosed = !document.querySelector('[data-slot="model-strength"]');
          revealSetting(ROUTING_PAGE_ROWS.strength.label);
        },
        () => {
          seen.strength = all('[data-slot="strength-row"]').map(text);
          asked.length = 0;
          button(ROUTING_PAGE_ROWS.refresh.label)?.click();
        },
        () => {
          // Refresh asks the agents and accounts again, the ratings, then reads the file again.
          seen.refreshAsked = [...new Set(asked)].toSorted();
          seen.unrevealable = Object.values(ROUTING_PAGE_ROWS)
            .map((row) => ("target" in row ? row.target : row.label))
            .filter(
              (label) =>
                !all("[data-setting]").some((element) => element.dataset.setting === label) &&
                !all("section[aria-labelledby] h2").some((heading) => text(heading) === label),
            );
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
      <Toaster className="top-2" />
      {result && <pre id="routing-result">{result}</pre>}
    </TooltipProvider>
  );
}

const root = document.getElementById("root");
if (root) createRoot(root).render(<Fixture />);
