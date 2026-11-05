/**
 * A standalone Vite entry, excluded from the app entry and production build: the one model picker
 * where it is used, with IPC mocked. `?shot=` leaves it open for a screenshot: codex (a started
 * Claude session, gpt-6.1-sol picked at Ultra, Fast), opencode (a model with no efforts), rule
 * (Opus 5.5 at High under "Never go above High effort"), search, automatic (Settings › Git), or
 * routing (a row of an order). `?drive=1` writes what it saw to #model-picker-result
 * (ModelPickerRender.test.ts reads it): the star, where it opens, a pick keeping it open with the
 * keyboard on the checked level (also when it comes back late, or with none checked), no efforts,
 * older models, and the search as plain typing.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useState } from "react";
import { createRoot } from "react-dom/client";

import { ConversationView } from "@/app/conversation/ConversationView";
import { GitPage } from "@/app/settings/GitPage";
import { OrderList } from "@/app/settings/routing/OrderList";
import { SidebarProvider } from "@/components/layout";
import { TooltipProvider } from "@/components/overlays";
import { model, overview } from "@/fixtures/agents";
import type { Conversation, ConversationView as View, GuidanceRow, ModelRule, ModelRules, Request, Settings } from "@/ipc/generated";
import { useModelGroups } from "@/lib/setup";
import { select } from "@/state/actions";
import { useApp } from "@/state/store";

const params = new URLSearchParams(location.search);
const shot = params.get("shot");
const driven = params.has("drive");

const id = "model-picker";
const start = Date.now() - 20 * 60_000;
let conversation = {
  id, kind: "session", projectId: null, title: "Tax helpers", pinnedAtMs: null, createdAtMs: start, updatedAtMs: start,
  lifecycle: "active", forkedFrom: null, sideOf: null, quotaWait: null,
  setup: {
    type: "session", orchestrator: { provider: "claude", model: "opus", effort: "high" }, permission: "acceptEdits", planMode: false,
    workersSeeUncommitted: null, environment: { type: "localCheckout", repoPath: "/repo", branch: null },
  },
} as unknown as Conversation;

const ceiling: ModelRule[] = [{ type: "effortCeiling", effort: "high" }];
const rules: ModelRules = { path: "/tmp/models.md", revision: "fixture", shipped: [], rows: [], rules: shot === "rule" ? ceiling : [], suggestions: [] } as unknown as ModelRules;

const asked: unknown[] = [];
const starred: Settings["favoriteModels"][] = [];
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  switch (req.method) {
    case "getConversation":
      return {
        method: req.method,
        view: {
          conversation, context: null, messages: { messages: [], hasMore: false }, tasks: [], approvals: [], questions: [],
          requests: [], workerSteps: [], orchestratorSteps: [], machineSteps: [], thinking: [],
          compactions: [], decisions: [], queue: { items: [], paused: false }, run: "idle",
          runRequest: null, head: null, ratings: {}, streaming: null, notices: [], previews: [],
        } as unknown as View,
      };
    case "getModelRules":
      return { method: req.method, rules };
    case "continueOn":
      asked.push({ provider: req.provider, model: req.model });
      return { method: req.method, when: "now" };
    case "updateSetup":
      // The daemon answers a moment later: the column shows the last model until then.
      return new Promise((resolve) => {
        setTimeout(() => {
          conversation = { ...conversation, setup: req.setup };
          resolve({ method: req.method, conversation });
        }, 150);
      });
    case "updateSettings":
      starred.push(req.settings.favoriteModels);
      return { method: req.method, settings: req.settings };
    default:
      return { method: req.method };
  }
});

const LEVELS = ["low", "medium", "high", "xhigh", "max"];
const claude = { efforts: LEVELS, defaultEffort: "medium" };
const codex = { efforts: [...LEVELS, "ultra"], defaultEffort: "medium" };
const FAST = "1.5x speed, increased usage";
const state = useApp.getState();
useApp.setState({
  conversations: { [id]: conversation },
  info: { ...state.info, platform: "macos" } as typeof state.info,
  connection: { status: "connected", daemon: null, reason: null },
  providers: {
    ...state.providers,
    view: {
      providers: [
        overview("claude", [
          model("opus", "Opus 5.5", { ...claude, isDefault: true, fast: FAST }),
          model("fable", "Fable 5.1", claude),
          model("sonnet", "Sonnet 5.5", claude),
          model("haiku", "Haiku 5.5", { efforts: ["low", "medium"], defaultEffort: "medium" }),
          ...(driven ? [model("opus-4-6", "Opus 4.6", { ...claude, legacy: true })] : []),
        ]),
        overview("codex", [
          model("gpt-6.1-sol", "gpt-6.1-sol", { ...codex, isDefault: true, fast: FAST }),
          model("gpt-6-astra", "gpt-6-astra", codex),
          model("gpt-6.1-terra", "gpt-6.1-terra", codex),
        ]),
        overview("opencode", [
          model("opencode/claude-opus-5-5", "opencode/claude-opus-5-5", claude),
          model("opencode-go/kimi-k3", "opencode-go/kimi-k3", { efforts: ["max"], defaultEffort: "max" }),
          model("opencode/big-pickle", "opencode/big-pickle"),
          // Its levels name no default of their own: none is checked.
          model("opencode-go/kimi-k3.1-pro", "opencode-go/kimi-k3.1-pro", { efforts: ["low", "medium", "high"] }),
        ]),
      ],
      sessions: [],
      fixtures: [],
    },
  },
});

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
async function until<T>(what: string, find: () => T | null | undefined | false, ms = 10_000): Promise<T> {
  const end = performance.now() + ms;
  for (;;) {
    const found = find();
    if (found) return found;
    if (performance.now() > end) throw new Error(`timed out waiting for ${what}`);
    await sleep(16);
  }
}
const all = (selector: string) => [...document.querySelectorAll<HTMLElement>(selector)];
const text = (element: Element | null | undefined) => element?.textContent?.trim() ?? "";
const trigger = () => document.querySelector<HTMLElement>("[data-slot=model-selector-trigger]");
const picker = () => document.querySelector<HTMLElement>("[data-slot=model-picker]");
const models = () => all("[data-slot=model-picker-model]");
const modelNamed = (name: string) => models().find((row) => text(row).startsWith(name));
const levels = () => all("[data-slot=model-picker-effort] [role=radio]");
const agent = (name: string) => document.querySelector<HTMLElement>(`[data-slot=model-picker-agent][data-agent=${name}]`);
const open = async (from: () => HTMLElement | null | undefined = trigger) => {
  (await until("the trigger", from)).click();
  await until("the picker", picker);
};
const browseAndPick = async (provider: string, name: string) => {
  (await until(provider, () => agent(provider))).click();
  (await until(name, () => modelNamed(name))).click();
};
const typeSearch = (words: string) => {
  const input = picker()!.querySelector("input")!;
  Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set?.call(input, words);
  input.dispatchEvent(new Event("input", { bubbles: true }));
  input.focus();
};

async function screenshot() {
  switch (shot) {
    case "codex":
      await open();
      await browseAndPick("codex", "gpt-6.1-sol");
      (await until("Ultra", () => levels().find((row) => text(row) === "Ultra"))).click();
      (await until("Fast", () => all("[role=radio]").find((row) => text(row) === "Fast"))).click();
      break;
    case "opencode":
      await open();
      await browseAndPick("opencode", "opencode/big-pickle");
      break;
    case "rule":
      await until("the rules read", () => useApp.getState());
      await sleep(300);
      await open();
      break;
    case "search":
      await open();
      typeSearch("o");
      break;
    case "automatic":
      await open(() => all("button").find((button) => button.getAttribute("aria-label") === "Write with"));
      break;
    case "routing":
      await open(() => document.querySelector<HTMLElement>("[data-slot=order-place-model]"));
      break;
  }
  await sleep(300);
  document.body.dataset.ready = "1";
}

async function drive() {
  const seen: Record<string, unknown> = {};
  try {
    await open();
    seen.opensOn = text(document.querySelector("[data-picker-part=agents] [aria-selected=true]"));
    seen.claude = models().map(text);
    seen.levels = levels().map((row) => [text(row), row.dataset.state]);
    // The star: it writes Settings, and the model goes first in its agent's list and on Favorites.
    all("[data-slot=model-picker-star]").find((star) => star.getAttribute("aria-label") === "Add Sonnet 5.5 to favorites")!.click();
    await until("the star written", () => starred.length === 1);
    seen.starred = starred[0];
    seen.starredFirst = await until("Sonnet first", () => (text(models()[0]) === "Sonnet 5.5" ? models().map(text) : null));
    trigger()!.click();
    await until("the picker closed", () => !picker());
    // A star on another model: it still opens on the checked model's agent.
    const opensOn = () => text(document.querySelector("[data-picker-part=agents] [aria-selected=true]"));
    await open();
    seen.otherStarredOpensOn = opensOn();
    // The search is plain typing: no autocorrect bubble.
    const field = picker()!.querySelector("input")!;
    seen.searchField = ["autocorrect", "autocapitalize", "spellcheck", "autocomplete", "writingsuggestions"].map((name) => field.getAttribute(name));
    // The checked model starred, it opens on Favorites.
    all("[data-slot=model-picker-star]").find((star) => star.getAttribute("aria-label") === "Add Opus 5.5 to favorites")!.click();
    await until("the second star written", () => starred.length === 2);
    trigger()!.click();
    await until("the picker closed again", () => !picker());
    await open();
    seen.checkedStarredOpensOn = opensOn();
    seen.favorites = models().map(text);
    // A pick keeps it open, and moves the keyboard to the picked model's levels.
    await browseAndPick("opencode", "opencode-go/kimi-k3");
    seen.afterPick = await until("the keyboard on its levels", () =>
      document.activeElement?.getAttribute("role") === "radio"
        ? { open: picker() !== null, focused: text(document.activeElement), levels: levels().map(text) }
        : null,
    );
    // A model with no efforts says so.
    await browseAndPick("opencode", "opencode/big-pickle");
    seen.noEfforts = await until("no efforts", () => (levels().length === 0 ? text(document.querySelector("[data-slot=model-picker-effort]")) : null));
    // The model under the keyboard shows its levels before it is picked.
    agent("codex")!.click();
    await until("Codex's models", () => modelNamed("gpt-6.1-sol"));
    const search = picker()!.querySelector("input")!;
    search.focus();
    search.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true, cancelable: true }));
    seen.underKeys = await until("its levels", () => (levels().length > 0 ? levels().map(text) : null));
    // Search hides the rail and finds every agent's models.
    typeSearch("o");
    seen.searchRail = agent("claude") !== null;
    seen.searched = await until("matches", () => (models().length > 1 ? models().map(text) : null));
    typeSearch("");
    agent("claude")!.click();
    // Older models fold.
    seen.older = await until("the fold", () => text(document.querySelector("[data-picker-older]")));
    document.querySelector<HTMLElement>("[data-picker-older]")!.click();
    seen.unfolded = await until("the older model", () => (modelNamed("Opus 4.6") ? models().map(text) : null));
    seen.asked = asked.length;
    // No level of the new model is checked: nothing looks selected, the keyboard waits on the column.
    await browseAndPick("opencode", "opencode-go/kimi-k3.1-pro");
    seen.noneChecked = await until("the keyboard on the column", () =>
      document.activeElement?.getAttribute("role") === "radiogroup"
        ? { focused: document.activeElement.getAttribute("aria-label"), checked: levels().filter((row) => row.dataset.state === "on").length, levels: levels().map(text) }
        : null,
    );
    // A pick that comes back late: the keyboard lands on its checked level, not the last model's.
    await browseAndPick("claude", "Haiku 5.5");
    seen.lateAnswer = await until("the late answer's level", () =>
      levels().length === 2 && document.activeElement?.getAttribute("role") === "radio"
        ? { focused: text(document.activeElement), state: (document.activeElement as HTMLElement).dataset.state, levels: levels().map(text) }
        : null,
    );
  } catch (error) {
    seen.error = String(error);
  }
  const out = document.createElement("pre");
  out.id = "model-picker-result";
  out.textContent = JSON.stringify(seen);
  document.body.append(out);
}

/** A kind of work's order on Settings › Routing, kept here as the page keeps it in the rules file. */
function Routing() {
  const groups = useModelGroups();
  const [order, setOrder] = useState<GuidanceRow[]>([
    { work: "Architecture and broad changes", agent: "claude", model: "opus", effort: "high", reason: "Your choice." },
    { work: "Architecture and broad changes", agent: "codex", model: "gpt-6.1-sol", effort: null, reason: "Your choice." },
  ]);
  return (
    <div className="bg-background text-foreground flex h-screen justify-center pt-12">
      <div className="bg-card border-divider rounded-settings h-fit w-full max-w-settings border px-4">
        <OrderList work="Architecture and broad changes" order={order} rules={ceiling} groups={groups} onSave={setOrder} />
      </div>
    </div>
  );
}

if (shot !== "automatic" && shot !== "routing") select({ type: "conversation", id });
if (shot === "automatic") {
  useApp.setState((s) => ({ settings: { ...s.settings, gitWriter: null } }));
}
createRoot(document.getElementById("root")!).render(
  <TooltipProvider>
    {shot === "automatic" ? (
      <div className="bg-background text-foreground flex h-screen">
        <GitPage />
      </div>
    ) : shot === "routing" ? (
      <Routing />
    ) : (
      <SidebarProvider className="bg-background flex h-screen flex-col" defaultOpen={false}>
        <main className="relative flex min-h-0 w-full flex-1 flex-col">
          <ConversationView key={id} selection={{ type: "conversation", id }} />
        </main>
      </SidebarProvider>
    )}
  </TooltipProvider>,
);
if (driven) void drive();
else void screenshot();
