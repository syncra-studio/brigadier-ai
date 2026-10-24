/** Settings › Git over a faked daemon: the writer model, the styles and their boxes, the prefix. */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { GitPage } from "@/app/settings/GitPage";
import { TooltipProvider } from "@/components/overlays";
import type { ModelInfo, ProviderKind, ProviderOverview, Request, Settings } from "@/ipc/generated";
import { useApp } from "@/state/store";

/*
 * ?drive=1 opens the model menu and picks Automatic, makes a picked model unavailable, picks
 * Custom instructions, types in its box (Save, ⌘S, Saved, then a save once typing pauses),
 * types a prefix git refuses and one it takes, and leaves a pull request box with an edit
 * (again while a save is on its way), then writes what it saw to #git-result (GitPageRender.test.ts reads it). ?custom=1 shows the
 * page with a picked model and custom commit instructions, for screenshots.
 */
const params = new URLSearchParams(location.search);
if (params.has("drive")) {
  const still = document.createElement("style");
  still.textContent = "*, *::before, *::after { animation: none !important; transition: none !important; }";
  document.head.append(still);
}

function model(id: string, displayName: string, fields: Partial<ModelInfo> = {}): ModelInfo {
  return {
    id,
    displayName,
    description: "",
    resolved: null,
    efforts: ["low", "medium", "high"],
    defaultEffort: "medium",
    isDefault: false,
    inputModalities: [],
    fast: null,
    legacy: false,
    ...fields,
  };
}

function overview(provider: ProviderKind, models: ModelInfo[]): ProviderOverview {
  return {
    provider,
    status: {
      provider,
      path: `/usr/local/bin/${provider}`,
      version: "2.1.0",
      loggedIn: true,
      authMethod: "subscription",
      plan: "max",
      guidance: null,
      compacts: true,
    },
    models: { provider, models, cliVersion: "2.1.0", fetchedAtMs: 0 },
    quota: null,
    usage: null,
    error: null,
    checkedAtMs: Date.now(),
  };
}

const custom = params.has("custom");
useApp.setState((state) => ({
  info: { ...state.info, platform: "macos" } as typeof state.info,
  connection: { ...state.connection, status: "connected" } as typeof state.connection,
  settings: {
    ...state.settings,
    ...(custom
      ? {
          gitWriter: { provider: "claude", model: "sonnet", effort: "low" },
          commitStyle: "custom",
          commitInstructions:
            "Start with the area in brackets, like [Settings]. Say what changed for the user, in the present tense. Keep the subject under 60 characters.",
          branchPrefix: "sg/",
        }
      : {}),
  },
  providers: {
    ...state.providers,
    view: {
      providers: [
        overview("claude", [model("opus", "Opus 5.5", { isDefault: true }), model("sonnet", "Sonnet 5.5")]),
        overview("codex", [model("gpt-6", "GPT-6", { isDefault: true }), model("gpt-6-luna", "GPT-6 Luna")]),
      ],
      sessions: [],
      fixtures: [],
    },
  },
}));

/** Every settings write, as the daemon got it: the fields this page owns. */
const writes: Partial<Settings>[] = [];
/** How long a settings write takes to land, in ms. */
let slow = 0;
mockIPC(async (command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  if (req.method === "updateSettings") {
    const { gitWriter, commitStyle, commitInstructions, prStyle, prInstructions, branchPrefix } = req.settings;
    writes.push({ gitWriter, commitStyle, commitInstructions, prStyle, prInstructions, branchPrefix });
    if (slow > 0) await new Promise((resolve) => setTimeout(resolve, slow));
    return { method: req.method, settings: req.settings };
  }
  return { method: req.method };
});

const all = <T extends Element = HTMLElement>(selector: string, root: ParentNode = document) => [
  ...root.querySelectorAll<T>(selector),
];
const text = (element: Element | null | undefined) => element?.textContent?.trim() ?? "";
const row = (label: string) => all('[data-slot="settings-row"]').find((element) => element.dataset.setting === label);
const button = (name: string, root: ParentNode = document) =>
  all<HTMLButtonElement>("button", root).find(
    (element) => text(element) === name || element.getAttribute("aria-label") === name,
  );
const openMenu = (label: string): "menu" => {
  const trigger = button(label);
  trigger?.focus();
  trigger?.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
  return "menu";
};
/** An open menu's items, or an open model picker's agents then models. */
const MENU_ITEMS = '[role="menu"][data-state="open"] [role^="menuitem"], [data-slot="model-picker"] [role="option"]';
const menuItems = () => all(MENU_ITEMS).map(text);
const pickMenu = (name: string) => all(MENU_ITEMS).find((item) => text(item).startsWith(name))?.click();
const setValue = (element: HTMLInputElement | HTMLTextAreaElement | null | undefined, value: string) => {
  if (!element) return;
  const proto = element instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
  Object.getOwnPropertyDescriptor(proto, "value")?.set?.call(element, value);
  element.dispatchEvent(new Event("input", { bubbles: true }));
};
const box = (label: string) => document.querySelector<HTMLTextAreaElement>(`textarea[aria-label="${label}"]`);
const boxEnd = (label: string) => text(box(label)?.parentElement?.lastElementChild);
const prefix = () => row("Branch prefix")?.querySelector("input");
const settings = () => useApp.getState().settings;

function Fixture() {
  const [result, setResult] = useState("");
  useEffect(() => {
    if (!params.has("drive")) return;
    const seen: Record<string, unknown> = {};
    const steps: (() => void | "menu" | Promise<void>)[] = [
      () => {
        seen.sections = all("main section h2").map(text);
        seen.rows = all('[data-slot="settings-row"]').map((element) => element.dataset.setting);
        seen.writer = text(button("Write with"));
        seen.writerLine = text(row("Write with")?.querySelector(".text-xs"));
        seen.boxes = all("textarea").length;
        // A picked model, then made unavailable on Providers.
        useApp.setState((state) => ({
          settings: {
            ...state.settings,
            gitWriter: { provider: "claude", model: "sonnet", effort: "low" },
          },
        }));
      },
      () => {
        seen.picked = text(button("Write with"));
        useApp.setState((state) => ({
          settings: { ...state.settings, hiddenModels: [{ provider: "claude", id: "sonnet" }] },
        }));
      },
      () => {
        seen.gone = text(row("Write with")?.querySelector(".text-xs"));
        return openMenu("Write with");
      },
      // Automatic is first among the picker's agents; it shows its one choice.
      () => {
        seen.writerAgents = all('[data-slot="model-picker-agent"]').map(text);
        pickMenu("Automatic");
      },
      () => {
        seen.writerMenu = all('[data-picker-part="models"] [role="option"]').map(text);
        document.querySelector<HTMLElement>('[data-slot="model-picker-automatic"]')?.click();
      },
      () => {
        seen.automatic = { trigger: text(button("Write with")), saved: settings().gitWriter };
        return openMenu("Commit messages");
      },
      () => {
        seen.styles = menuItems();
        pickMenu("Custom instructions");
      },
      () => {
        seen.commitStyle = settings().commitStyle;
        seen.boxShown = box("Commit instructions") !== null;
        seen.placeholder = box("Commit instructions")?.placeholder;
        setValue(box("Commit instructions"), "Start with [Area].");
      },
      () => {
        seen.dirty = boxEnd("Commit instructions");
        box("Commit instructions")?.dispatchEvent(
          new KeyboardEvent("keydown", { key: "s", metaKey: true, bubbles: true, cancelable: true }),
        );
      },
      () => {
        seen.afterCmdS = { end: boxEnd("Commit instructions"), saved: settings().commitInstructions };
        setValue(box("Commit instructions"), "Start with [Area]. Present tense.");
      },
      async () => {
        // Typing pauses: it saves by itself after 3 s, not before.
        await new Promise((resolve) => setTimeout(resolve, 1500));
        seen.beforePause = settings().commitInstructions;
        await new Promise((resolve) => setTimeout(resolve, 2000));
        seen.afterPause = { end: boxEnd("Commit instructions"), saved: settings().commitInstructions };
        await new Promise((resolve) => setTimeout(resolve, 3200));
        seen.quiet = boxEnd("Commit instructions");
      },
      () => {
        prefix()?.focus();
        setValue(prefix(), "team..x/");
      },
      () => {
        seen.refused = text(row("Branch prefix")?.querySelector('[role="alert"]'));
        prefix()?.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
      },
      () => {
        seen.prefixKept = settings().branchPrefix;
        setValue(prefix(), "sg/");
      },
      () => prefix()?.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true })),
      () => {
        seen.prefixSaved = settings().branchPrefix;
        return openMenu("Pull requests");
      },
      () => pickMenu("Custom instructions"),
      () => setValue(box("Pull request instructions"), "Title under 70 characters."),
      // Another style while the box has an edit: what was typed is saved as the box goes.
      () => openMenu("Pull requests"),
      () => pickMenu("Conventional Commits"),
      async () => {
        await new Promise((resolve) => setTimeout(resolve, 300));
        seen.pr = { style: settings().prStyle, instructions: settings().prInstructions, box: box("Pull request instructions") !== null };
        // Again with a save on its way when the box goes: the edit typed meanwhile follows it.
        slow = 800;
        return openMenu("Pull requests");
      },
      () => pickMenu("Custom instructions"),
      () => setValue(box("Pull request instructions"), "Title under 70 characters. Body in bullets."),
      () => {
        box("Pull request instructions")?.dispatchEvent(
          new KeyboardEvent("keydown", { key: "s", metaKey: true, bubbles: true, cancelable: true }),
        );
      },
      () => setValue(box("Pull request instructions"), "Title under 70 characters. Body in bullets. No emoji."),
      () => openMenu("Pull requests"),
      () => pickMenu("Conventional Commits"),
      async () => {
        await new Promise((resolve) => setTimeout(resolve, 3000));
        seen.prDuringSave = settings().prInstructions;
        seen.writes = writes.length;
        setResult(JSON.stringify(seen));
      },
    ];
    let stopped = false;
    const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
    const menuOpen = () => document.querySelector('[role="menu"][data-state="open"], [data-slot="model-picker"]') !== null;
    void (async () => {
      await wait(400);
      for (const step of steps) {
        if (stopped) return;
        const done = step();
        if (done instanceof Promise) await done;
        await wait(120);
        if (done === "menu") for (let tries = 0; tries < 40 && !menuOpen(); tries++) await wait(50);
      }
    })();
    return () => {
      stopped = true;
    };
  }, []);
  return (
    <TooltipProvider>
      <div className="bg-background text-foreground flex h-screen">
        <main className="flex h-full min-w-0 flex-1">
          <div className="min-w-0 flex-1">
            <GitPage />
          </div>
        </main>
      </div>
      {result && <pre id="git-result">{result}</pre>}
    </TooltipProvider>
  );
}

const root = document.getElementById("root");
if (root) createRoot(root).render(<Fixture />);
