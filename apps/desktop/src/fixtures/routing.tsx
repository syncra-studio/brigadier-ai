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
import { IDLE_RESEARCH } from "@/fixtures/rankings";
import type { GuidanceRow, ModelRule, ModelRules, RankingsRefresh, Request, RoutingSuggestion } from "@/ipc/generated";
import { loadModelRules } from "@/state/modelRules";
import { openSettings } from "@/state/actions";
import { useApp } from "@/state/store";
import { useToasts } from "@/state/toasts";

/*
 * ?drive=1 reads who does what and the rules from the fixture's rules file, opens a shipped kind
 * and makes it My order, changes its effort, adds a model, moves it up and removes both, adds
 * rules in its own words (examples fill the box, an error, Brigadier rewriting them, Cancel,
 * what it will save, Add, Undo), edits the file by hand and opens the page again (an order, rules
 * for one work, a cap and a minimum that stop a kind, advice), then opens How a model is picked
 * through search and presses Refresh models, writing what it saw (and what the page wrote to the file) to #routing-result
 * (RoutingPageRender.test.ts reads it). Without it the page stays as it opens, for screenshots:
 * ?kind=<name> opens a kind, ?more=strength|steps opens a row of More, ?rules=1|2|3|4 shows the
 * Rules section (its rules with a warning and advice; Add rule opened; what Brigadier will save;
 * Brigadier rewriting the words). ?drive=role-suggestions takes the better model offered to a
 * review role, then to a fusion consult. ?drive=look reads Settings › General's card, select
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
  select: surface(root.querySelector('[data-slot="settings-page"] button[aria-haspopup]')),
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
  { work: "Adversarial review", agent: "codex", model: "default", effort: "high", reason: "This provider's reviewer; `--reviewing` takes the row on another provider than the author's." },
  { work: "Adversarial review", agent: "claude", model: "opus", effort: "high", reason: "This provider's reviewer; `--reviewing` takes the row on another provider than the author's." },
  { work: "fusion-claude", agent: "claude", model: "opus", effort: "high", reason: "A fusion's Claude consult: the strongest Claude model." },
  { work: "fusion-codex", agent: "codex", model: "default", effort: "high", reason: "A fusion's Codex consult: the CLI's own default model." },
  { work: "Computer use", agent: "claude", model: "opus", effort: "high", reason: "Seeing and using apps on screen takes strong judgment." },
  { work: "Computer use", agent: "codex", model: "default", effort: "high", reason: "The next choice when the first can't run." },
  { work: "Fallback", agent: "claude", model: "opus", effort: "high", reason: "A session that moves to Claude Code: when it is asked to, or every account of its own agent is at its limit." },
  { work: "Fallback", agent: "codex", model: "default", effort: "high", reason: "A session that moves to Codex: when it is asked to, or every account of its own agent is at its limit." },
  { work: "Conversation titles", agent: "codex", model: "luna", effort: "low", reason: "A small model for short text; use available quota, cost tier and suitability among shipped title choices." },
  { work: "Conversation titles", agent: "claude", model: "haiku", effort: null, reason: "A small model for short text; use available quota, cost tier and suitability among shipped title choices." },
  { work: "Rule reading", agent: "claude", model: "sonnet", effort: "medium", reason: "Rewrites the rules you type into exact rules and advice." },
  { work: "Rule reading", agent: "codex", model: "default", effort: "medium", reason: "The next choice when the first can't run." },
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
/** ?drive=role-suggestions: your models for review and Fusion, each Claude role on Sonnet; a Fallback row written in lower case. */
const roleSuggestions = params.get("drive") === "role-suggestions";
if (roleSuggestions) {
  file.rows = [
    { work: "Adversarial review", agent: "codex", model: "default", effort: "high", reason: "Mine." },
    { work: "Adversarial review", agent: "claude", model: "sonnet", effort: "medium", reason: "Mine." },
    { work: "fusion-claude", agent: "claude", model: "sonnet", effort: "medium", reason: "Mine." },
    { work: "fusion-codex", agent: "codex", model: "default", effort: "high", reason: "Mine." },
    { work: "fallback", agent: "codex", model: "gpt-6", effort: "low", reason: "Mine." },
    { work: "Release notes", agent: "codex", model: "gpt-6", effort: "low", reason: "Short and cheap." },
  ];
}
/** ?rules=: the Rules section's screens, over the rules of the approved mockup. */
const scene = params.get("rules");
if (scene) {
  file.rules = [
    { type: "forbid", names: ["fable"] },
    { type: "effortCeiling", effort: "high" },
    { type: "effortCeiling", effort: "medium", model: "opus" },
    { type: "effortFloor", effort: "high", work: "Adversarial review" },
    // Before Add, the list doesn't hold the advice the box is about to save (as the mockup shows).
    ...(scene === "3" || scene === "4" ? [] : [{ type: "advice" as const, text: "Prefer Codex when Claude is busy" }]),
  ];
}
/** The words the screens and the drive type, and what the faked Rule reading makes of them. */
const TYPED = "never go over high effort and keep sonnet off reviews. prefer codex when claude is busy";
const READ: ModelRule[] = [
  { type: "effortCeiling", effort: "high" },
  { type: "forbid", names: ["sonnet"], work: "Adversarial review" },
  { type: "advice", text: "Prefer Codex when Claude is busy" },
];
/** A second Add's words, and its rule: added after the first, it outlives the first one's Undo. */
const TYPED_LATER = "never use haiku";
const READ_LATER: ModelRule[] = [{ type: "forbid", names: ["haiku"] }];
/** How long the faked Rule reading takes; scene 4 never ends, to show it rewriting. */
const READING_MS = scene === "4" ? 3_600_000 : 900;
/** The ratings' view of Bounded implementation: GPT-6 rates above the Sonnet 5.5 the user put first. */
const rated = { score: 9, current: 8.5 };
/** The daemon's suggestion: only for an order whose first is Sonnet, and not dismissed at this rating. */
const suggestions = (): RoutingSuggestion[] => {
  if (roleSuggestions) {
    return file.rows
      .filter((row) => row.agent === "claude" && row.model === "sonnet")
      .map(({ work }) => ({ work, row: { work, agent: "claude", model: "opus", effort: "high", reason: "Rated 9.1 of 10 for this work." }, score: 9.1, current: "sonnet", currentScore: 8.5 }));
  }
  const first = file.rows.find((row) => row.work === "Bounded implementation");
  const dismissed = useApp
    .getState()
    .settings.dismissedSuggestions.some((gone) => gone.work === "Bounded implementation" && gone.model === "gpt-6" && gone.score === rated.score);
  if (first?.model !== "sonnet" || dismissed) return [];
  const row: GuidanceRow = { work: "Bounded implementation", agent: "codex", model: "gpt-6", effort: "high", reason: "Rated 9.0 of 10 for this work." };
  return [{ work: row.work, row, score: rated.score, current: "sonnet", currentScore: rated.current }];
};
/** The last ratings research: three hours ago, on Sonnet 5.5. */
const research: RankingsRefresh = {
  ...IDLE_RESEARCH,
  state: "done",
  jobId: "research-1",
  startedAtMs: Date.now() - 3 * 3_600_000 - 60_000,
  finishedAtMs: Date.now() - 3 * 3_600_000,
  model: "sonnet",
  provider: "claude",
  changes: [{ provider: "codex", model: "gpt-6", fields: ["strengths.implement"], sources: ["https://example.com/gpt-6"] }],
  found: ["Bounded implementation"],
  overlayApplied: true,
};
const read = (): ModelRules => ({
  path: "/Users/me/.brigadier/models.md",
  revision: JSON.stringify(file),
  shipped: SHIPPED,
  rows: [...file.rows],
  rules: [...file.rules],
  suggestions: suggestions(),
  research,
});
/** A model the ratings don't score yet: its strength is only a placeholder. */
const unrated = (() => {
  const base = usageView().models.find((model) => model.id === "claude-opus-4-6")!;
  return { ...base, id: "claude-new", registryKey: "claude-new", displayName: "Claude New", tier: "unrated" as const, legacy: false };
})();
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
      return { method: req.method, usage: { ...usageView(), registry: { ...usageView().registry, updated: "2026-10-21", checkedAtMs: Date.now() - 3 * 3_600_000 - 600_000 }, models: [...usageView().models, unrated] } };
    case "refreshAccounts":
      return { method: req.method, generation: 0 };
    case "checkRegistry":
      return { method: req.method, registry: usageView().registry };
    case "getModelRules":
      reads += 1;
      return { method: req.method, rules: read() };
    case "setModelRules":
      if (req.revision !== read().revision) throw new Error("Routing changed since this page was read.");
      writes.push({ rows: req.rows, rules: req.rules });
      file.rows = req.rows;
      file.rules = req.rules;
      return { method: req.method, rules: read() };
    case "readModelRules":
      return new Promise((resolve, reject) =>
        setTimeout(
          () =>
            req.text.includes("gibberish")
              ? reject({ code: "invalid", message: "Brigadier couldn't make a rule of that: try other words." })
              : resolve({ method: req.method, rules: req.text.includes("haiku") ? READ_LATER : READ }),
          READING_MS,
        ),
      );
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
/** A kind of work's row in Who does what, by its name. */
const kindButton = (name: string) =>
  all<HTMLButtonElement>('[data-slot="routing-kind"] > button').find((element) => text(element).startsWith(name));
const openKind = () => document.querySelector('[data-slot="routing-kind"][data-open]');
const roleRoot = () => openKind()?.querySelector('[data-slot="role-picks"]') ?? document;
const roleOrder = () => all('[data-slot="ranked-place"]', roleRoot());
const kinds = () => all('[data-slot="routing-kind"] > button').map(text);
const ruleRows = () => all('[data-slot="routing-rule"]').map(text);
const form = () => document.querySelector('[data-slot="routing-rule-form"]');
const box = () => form()?.querySelector("textarea") ?? null;
/** Types `words` into the rule box as a user would (React reads the native input event). */
const type = (words: string) => {
  const area = box();
  if (!area) return;
  Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")?.set?.call(area, words);
  area.dispatchEvent(new Event("input", { bubbles: true }));
};
const note = () => text(form()?.querySelector('[role="status"], [role="alert"], [data-slot="routing-rule-read"]'));
const lastWrite = () => writes.at(-1);
/** The open model picker's agent row, by its name. */
const agentRow = (name: string) => all<HTMLElement>('[data-slot="model-picker-agent"]').find((row) => text(row) === name);
/** The open model picker's model rows: name, then line. */
const modelRows = () => all('[data-slot="model-picker-model"]').map(text);
const modelRow = (name: string) => all<HTMLElement>('[data-slot="model-picker-model"]').find((row) => text(row).startsWith(name));
/** The open model picker's effort rows, and one by its name. */
const effortRows = () => all('[data-slot="model-picker-effort"] [role="radio"]');
const effortRow = (name: string) => effortRows().find((row) => text(row).startsWith(name));
/** The steps that show the Rules section at `stage`, for screenshots. */
const roleSuggestionLine = (work: string) => document.querySelector(`[data-work="${work}"] [data-slot="routing-suggestion"]`);

function ruleSteps(stage: string): (() => void)[] {
  const steps: (() => void)[] = [() => document.querySelector('[data-slot="routing-rules"]')?.closest("section")?.scrollIntoView({ block: "start" })];
  if (stage === "1") return steps;
  steps.push(() => (document.querySelector('[data-slot="add-rule"]') as HTMLButtonElement | null)?.click());
  if (stage === "2") return steps;
  steps.push(() => type(TYPED), () => button("Add", form() ?? document)?.click());
  // Stage 3 waits for the reading to end; stage 4's never does.
  if (stage === "3") steps.push(() => {}, () => {}, () => {}, () => {});
  return steps;
}

function Fixture() {
  const page = useApp((s) => (s.selection.type === "settings" ? s.selection.page : null));
  const [result, setResult] = useState("");
  useEffect(() => {
    openSettings("routing");
    const seen: Record<string, unknown> = {};
    // The Undo of each of the drive's two Adds, pressed later.
    let undoFirst: (() => void) | undefined;
    let undoLater: (() => void) | undefined;
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
        ...(scene ? ruleSteps(scene) : []),
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
    } else if (roleSuggestions) {
      steps = [
        () => {
          seen.lines = all('[data-slot="routing-suggestion"]').map(text);
          seen.kinds = kinds();
          seen.writesBefore = writes.length;
          button("Use it", roleSuggestionLine("Adversarial review") ?? document)?.click();
        },
        () => {
          seen.afterReview = [...file.rows];
          seen.linesAfterReview = all('[data-slot="routing-suggestion"]').map(text);
          button("Use it", roleSuggestionLine("Fusion") ?? document)?.click();
        },
        () => {
          seen.afterFusion = [...file.rows];
          seen.writes = writes.length;
          seen.linesAfterFusion = all('[data-slot="routing-suggestion"]').length;
          setResult(JSON.stringify(seen));
        },
      ];
    } else if (params.get("drive") === "roles") {
      const roles = ["Adversarial review", "Fusion", "Fallback"];
      seen.roles = [];
      steps = roles.flatMap((work) => {
        const record: Record<string, unknown> = { work };
        return [
          () => kindButton(work)?.click(),
          () => {
            record.closedHasModel = Boolean(kindButton(work)?.querySelector('[data-slot="now-pick"]'));
            record.automatic = all('[data-slot="role-pick"]', roleRoot()).map(text);
            record.note = text(roleRoot().querySelector("p"));
            record.noFallbackBadges = !roleRoot().textContent?.includes("Picked now") && !roleRoot().textContent?.includes("Next if");
            button("My order", openKind() ?? document)?.click();
          },
          () => {
            record.initial = lastWrite()?.rows;
            record.canReorder = Boolean(roleRoot().querySelector('button[aria-label^="Move "]'));
            roleOrder()[0]?.querySelector<HTMLElement>('[data-slot="order-place-model"]')?.click();
          },
          () => effortRow("Medium")?.click(),
          () => {
            record.afterEffort = lastWrite()?.rows;
            record.agents = all('[data-slot="model-picker-agent"]').map(text);
            modelRow(work === "Adversarial review" ? "GPT-6 Luna" : "Sonnet 5.5")?.click();
          },
          () => {
            record.afterFirstModel = lastWrite()?.rows;
            roleOrder()[0]?.querySelector<HTMLElement>('[data-slot="order-place-model"]')?.click();
          },
          () => roleOrder()[1]?.querySelector<HTMLElement>('[data-slot="order-place-model"]')?.click(),
          () => effortRow("Low")?.click(),
          () => {
            record.afterSecondEffort = lastWrite()?.rows;
            modelRow(work === "Adversarial review" ? "Sonnet 5.5" : "GPT-6 Luna")?.click();
          },
          () => {
            record.afterBothModels = lastWrite()?.rows;
            roleOrder()[1]?.querySelector<HTMLElement>('[data-slot="order-place-model"]')?.click();
          },
          () => {
            record.labelsAfterEdit = roleOrder().map(text);
            button("Automatic", openKind() ?? document)?.click();
          },
          () => {
            record.afterReset = lastWrite()?.rows;
            record.reset = all('[data-slot="role-pick"]', roleRoot()).map(text);
            (seen.roles as unknown[]).push(record);
          },
        ];
      });
      steps.push(
        () => kindButton("Conversation titles")?.click(),
        () => {
          seen.noEffort = all('[data-slot="auto-pick"]', openKind() ?? document).map(text);
          kindButton("Computer use")?.click();
        },
        () => {
          seen.fallbackOrder = all('[data-slot="auto-pick"]', openKind() ?? document).map(text);
          revealSetting(ROUTING_PAGE_ROWS.refresh.label);
        },
        () => {
          seen.refreshOpensStrength = Boolean(document.querySelector('[data-slot="model-strength"]'));
          seen.refreshInHeader = Boolean(document.querySelector('header [data-setting="Refresh models"]'));
          setResult(JSON.stringify(seen));
        },
      );
    } else {
      steps = [
        () => {
          seen.note = text(document.querySelector('[data-slot="settings-page"] header'));
          seen.sections = all("section h2").map(text);
          seen.sources = text(document.querySelector('[data-slot="who-does-what"]')?.closest("section")?.querySelector("p"));
          seen.rulesDescription = text(document.querySelector('[data-slot="routing-rules"]')?.closest("section")?.querySelector("p"));
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
          button("My order", openKind() ?? document)?.click();
        },
        () => {
          seen.afterMine = lastWrite()?.rows;
          seen.mine = all('[data-slot="ranked-place"]', openKind() ?? document).map(text);
          seen.aloneGripDisabled = all<HTMLButtonElement>('[data-slot="ranked-place"] button', openKind() ?? document)[0]?.disabled;
          button("Opus 5.5: model and effort")?.click();
        },
        () => {
          // What the user's ceiling keeps out shows greyed, with its words.
          seen.efforts = effortRows().map(text);
          effortRow("Medium")?.click();
        },
        () => {
          seen.afterEffort = lastWrite()?.rows.find((row) => row.work === "Architecture and broad changes");
          button("Opus 5.5: model and effort")?.click();
        },
        () => button("Add model", openKind() ?? document)?.click(),
        () => {
          seen.addModelAgents = all('[data-slot="model-picker-agent"]').map(text);
          seen.addModelItems = { claude: modelRows() };
          agentRow("Codex")?.click();
        },
        () => {
          (seen.addModelItems as Record<string, string[]>).codex = modelRows();
          modelRow("GPT-6")?.click();
        },
        // A pick keeps it open; Add model closes it.
        () => button("Add model", openKind() ?? document)?.click(),
        () => {
          seen.order = all('[data-slot="ranked-place"]', openKind() ?? document).map(text);
          seen.kindLine = text(kindButton("Architecture and broad changes"));
          // GPT-6 moved up a place from its grip, by the keyboard.
          button("Move GPT-6 (2 of 2); drag, or press up and down")?.dispatchEvent(
            new KeyboardEvent("keydown", { key: "ArrowUp", bubbles: true }),
          );
        },
        () => {
          seen.afterMove = lastWrite()?.rows.filter((row) => row.work === "Architecture and broad changes").map((row) => row.model);
          button("GPT-6: model and effort")?.click();
        },
        // Its picker opens on its own agent; the keys stay in it and a pick keeps it open.
        () => {
          const search = document.querySelector<HTMLInputElement>('[data-slot="model-picker"] input');
          seen.changeModel = {
            searchFocused: document.activeElement === search,
            codex: modelRows(),
          };
          // ↓ into the models, ← to the agents, ↑ browses Claude Code, then typing searches.
          for (const key of ["ArrowDown", "ArrowLeft", "ArrowUp", "s"]) {
            document.activeElement?.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));
          }
        },
        () => {
          const changeModel = seen.changeModel as Record<string, unknown>;
          changeModel.typed = document.querySelector<HTMLInputElement>('[data-slot="model-picker"] input')?.value;
          changeModel.searched = modelRows();
          document.activeElement?.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
        },
        () => {
          const changeModel = seen.changeModel as Record<string, unknown>;
          changeModel.afterEscape = { open: Boolean(document.querySelector('[data-slot="model-picker"]')), typed: document.querySelector<HTMLInputElement>('[data-slot="model-picker"] input')?.value };
          modelRow("Sonnet 5.5")?.click();
        },
        () => {
          const changeModel = seen.changeModel as Record<string, unknown>;
          changeModel.stillOpen = Boolean(document.querySelector('[data-slot="model-picker"]'));
          changeModel.order = all('[data-slot="ranked-place"]', openKind() ?? document).map(text);
          button("Sonnet 5.5: model and effort")?.click();
        },
        () => button("Remove Sonnet 5.5")?.click(),
        () => button("Remove Opus 5.5")?.click(),
        () => {
          // Its last model removed, the kind is Automatic again.
          seen.afterRemove = lastWrite()?.rows.map((row) => row.work);
          seen.modeAfterRemove = text(openKind()?.querySelector('[data-state="on"]'));
          kindButton("Architecture and broad changes")?.click();
          (document.querySelector('[data-slot="add-rule"]') as HTMLButtonElement | null)?.click();
        },
        () => {
          seen.form = text(form());
          seen.addDisabled = button("Add", form() ?? document)?.disabled;
          seen.headerAddShown = Boolean(document.querySelector('[data-slot="add-rule"]'));
          button("Never use Sonnet", form() ?? document)?.click();
        },
        () => button("Never go above High effort", form() ?? document)?.click(),
        () => {
          // Examples fill the box, one a line.
          seen.filled = box()?.value;
          type("gibberish");
        },
        () => button("Add", form() ?? document)?.click(),
        () => {
          seen.reading = note();
          seen.readingButton = [text(button("Reading…", form() ?? document)), button("Reading…", form() ?? document)?.disabled, box()?.disabled];
          seen.readsAsked = asked.filter((method) => method === "readModelRules").length;
        },
        () => {},
        () => {},
        () => {
          // An error is one plain line; the words stay to change.
          seen.readError = note();
          seen.keptWords = box()?.value;
          type(TYPED);
        },
        () => button("Add", form() ?? document)?.click(),
        () => {
          seen.readingAgain = note();
          button("Cancel", form() ?? document)?.click();
        },
        () => {
          // Cancel while it rewrites stops waiting and keeps the words.
          seen.afterCancel = { open: Boolean(form()), words: box()?.value, note: note() };
          button("Add", form() ?? document)?.click();
        },
        () => {},
        () => {},
        () => {},
        () => {
          // A reply to the cancelled reading changed nothing; this one shows what Brigadier will save.
          seen.read = note();
          seen.writesBeforeAdd = writes.length;
          button("Add", form() ?? document)?.click();
        },
        () => {
          seen.formAfterAdd = Boolean(form());
          seen.writtenRules = lastWrite()?.rules;
          seen.rulesAfterAdd = ruleRows();
          const added = useToasts.getState().toasts.at(-1);
          seen.toast = added && [added.text, ...added.actions.map((action) => action.label)];
          undoFirst = added?.actions[0]?.run;
          // A second rule added before the first one's Undo is pressed (#757).
          (document.querySelector('[data-slot="add-rule"]') as HTMLButtonElement | null)?.click();
        },
        () => type(TYPED_LATER),
        () => button("Add", form() ?? document)?.click(),
        () => {},
        () => {},
        () => {},
        () => button("Add", form() ?? document)?.click(),
        () => {
          seen.rulesAfterLaterAdd = ruleRows();
          undoLater = useToasts.getState().toasts.at(-1)?.actions[0]?.run;
          undoFirst?.();
        },
        () => {
          // The first Undo took out only its own rules: the later one stays.
          seen.rulesAfterFirstUndo = ruleRows();
          undoLater?.();
        },
        () => {
          seen.rulesAfterUndo = ruleRows();
          // A hand edit of the file while the page is away, then the page opened again.
          file.rows = [
            ...file.rows,
            { work: "Docs", agent: "claude", model: "sonnet", effort: "low", reason: "Hand edit." },
            { work: "Docs", agent: "codex", model: "gpt-6", effort: null, reason: "Hand edit." },
          ];
          file.rules = [
            ...file.rules,
            { type: "allow", agent: "codex", names: ["gpt-6"] },
            { type: "forbid", names: ["opus"], work: "Architecture and broad changes" },
            { type: "forbid", names: ["sonnet"], work: "Docs" },
            { type: "effortCeiling", effort: "medium", model: "opus" },
            { type: "effortFloor", effort: "high", work: "Adversarial review" },
            { type: "advice", text: "Prefer Codex when Claude is busy" },
          ];
          openSettings("general");
        },
        () => openSettings("routing"),
        () => {
          seen.kindsAfterHandEdit = kinds();
          seen.rulesAfterHandEdit = ruleRows();
          seen.warnings = all('[data-slot="routing-rule-warning"]').map(text);
          // × removes a rule.
          button("Remove “Prefer Codex when Claude is busy”")?.click();
          kindButton("Architecture and broad changes")?.click();
        },
        () => {
          // Your rule for that work keeps the shipped model out: Automatic says so.
          seen.skipped = text(openKind()?.querySelector('[data-slot="auto-picks"]'));
          seen.afterRemoveRule = lastWrite()?.rules.map((rule) => rule.type);
          revealSetting(ROUTING_PAGE_ROWS.steps.label);
        },
        () => {
          seen.steps = all('[data-slot="routing-steps"] li').map(text);
          seen.stepsExample = text(document.querySelector('[data-slot="routing-steps-example"]'));
          seen.strengthClosed = !document.querySelector('[data-slot="model-strength"]');
          revealSetting(ROUTING_PAGE_ROWS.strength.label);
        },
        () => {
          // A better-rated model for the order of yours: a line on its row, nothing written.
          seen.researchRow = text(document.querySelector('[data-slot="ratings-research"]'));
          seen.suggestion = text(document.querySelector('[data-work="Bounded implementation"] [data-slot="routing-suggestion"]'));
          seen.suggestionKinds = all('[data-slot="routing-suggestion"]').length;
          seen.writesBeforeSuggestion = writes.length;
          button("Dismiss", document.querySelector('[data-slot="routing-suggestion"]') ?? document)?.click();
        },
        () => {
          // Dismissed: hidden at this rating, your order as it was.
          seen.dismissed = useApp.getState().settings.dismissedSuggestions;
          seen.suggestionAfterDismiss = all('[data-slot="routing-suggestion"]').length;
          seen.writesAfterDismiss = writes.length;
          // The rating changes again: it is back.
          rated.score = 9.5;
          void loadModelRules();
        },
        () => {
          seen.suggestionBack = text(document.querySelector('[data-slot="routing-suggestion"]'));
          button("Use it", document.querySelector('[data-slot="routing-suggestion"]') ?? document)?.click();
        },
        () => {
          // Use it puts it first in your order; that click is what writes it.
          seen.writesAfterUse = writes.length;
          seen.orderAfterUse = file.rows.filter((row) => row.work === "Bounded implementation").map((row) => `${row.agent}/${row.model}/${row.effort}`);
          seen.suggestionAfterUse = all('[data-slot="routing-suggestion"]').length;
        },
        () => {
          seen.strength = all('[data-slot="strength-row"]').map(text);
          asked.length = 0;
          button(ROUTING_PAGE_ROWS.refresh.label)?.click();
        },
        () => {
          // Refresh asks the agents and accounts again, the ratings, then reads the file again.
          // The Plugins catalog polls independently of Refresh models.
          seen.refreshAsked = [...new Set(asked.filter((method) => method !== "getPluginCatalog"))].toSorted();
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
