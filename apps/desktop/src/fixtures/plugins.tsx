/** Settings › Plugins over a faked daemon: every list, details and dialog state the page draws. No sample data enters the app. */
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { SettingsNav } from "@/app/settings/SettingsNav";
import { GeneralPage } from "@/app/settings/GeneralPage";
import { connectPlugins, usePlugins, type PluginAgentResult, type PluginsAdapter } from "@/app/settings/plugins/adapter";
import { PluginsPage } from "@/app/settings/plugins/PluginsPage";
import type { PluginItemDetails, PluginItemView, PluginProjectView, PluginsView } from "@/app/settings/plugins/types";
import { SidebarPanel, SidebarProvider } from "@/components/layout";
import { Toaster, TooltipProvider } from "@/components/overlays";
import { SettingsTitlebarStrip, TerminalButton, TitlebarNav } from "@/components/shared";
import type { ProviderKind } from "@/ipc/generated";
import { useApp } from "@/state/store";
import { openSettings, stepHistory } from "@/state/actions";
import { useHistory } from "@/state/history";

/*
 * ?state=<name> opens one state and writes what it shows to #plugins-result: L1 (MCP servers),
 * L2 (Skills), D1 (a server's details: sign-in, only some projects), D2 (a skill's details), D3 (an
 * item only Claude Code has, removed earlier), D4 (a locked item), F1 (a skill's SKILL.md open),
 * F2 (a server's settings open), R1 (Remove of an item an agent has of its own), A1 (Add).
 * ?width=<px> narrows the window. ?drive=1 walks the states, flips, scopes, edits and saves, then
 * writes what it saw (PluginsRender.test.ts reads it); ?drive=removal walks a removal that
 * answers late (PluginRemovalRender.test.ts).
 */
const params = new URLSearchParams(location.search);
const deferredRemoval = params.get("drive") === "removal";
const width = Number(params.get("width")) || null;
let finishRemoval: (() => void) | null = null;
let reads = 0;
const still = document.createElement("style");
still.textContent = "*, *::before, *::after { animation: none !important; transition: none !important; }";
document.head.append(still);
useApp.setState((state) => ({ info: { ...state.info, platform: "macos" } as typeof state.info, selection: { type: "settings", page: "plugins" } }));

const AGENTS: ProviderKind[] = ["claude", "codex"];
const HOME = "/Users/me";
const LIB = `${HOME}/.brigadier/library/plugins`;
const projects: PluginProjectView[] = [
  { id: "p-brigadier", name: "brigadier-ai", path: `${HOME}/Code/brigadier-ai`, notes: [] },
  { id: "p-steady", name: "steady-app", path: `${HOME}/Code/steady-app`, notes: [] },
  { id: "p-landing", name: "landing", path: `${HOME}/Code/landing`, notes: [{ agent: "codex", message: "Codex only reads this project's settings once you trust the project." }] },
  { id: "p-docs", name: "docs-site", path: `${HOME}/Code/docs-site`, notes: [] },
];

const some = (...projectIds: string[]) => ({ type: "someProjects" as const, projectIds, folders: [] });
/** One item as the daemon sends it, from what sets it apart. */
function item(spec: Pick<PluginItemView, "key" | "kind" | "name" | "detail"> & Partial<PluginItemView>): PluginItemView {
  return {
    description: "", url: null, on: true, scope: { type: "allProjects" }, problem: null, locked: null, removal: "everything", canShare: false,
    skillCount: spec.kind === "skill" ? 1 : 0, files: [spec.kind === "skill" ? `${LIB}/${spec.name}/skills/${spec.name}/SKILL.md` : `${LIB}/${spec.name}/.mcp.json`],
    ...spec,
  };
}

function startingItems(): PluginItemView[] {
  return [
    // Its site never answers (tests make no outside requests): its card falls back to the illustration.
    item({ key: "lib:context7", kind: "server", name: "Context7", description: "Up-to-date docs and code examples for the libraries you use.", detail: "https://mcp.context7.invalid/mcp", url: "https://mcp.context7.invalid/mcp" }),
    item({ key: "lib:figma", kind: "server", name: "Figma", description: "Frames, components and variables from your Figma files.", detail: "http://127.0.0.1:3845/mcp", url: "http://127.0.0.1:3845/mcp" }),
    item({
      key: "server:github", kind: "server", name: "GitHub", description: "Issues, pull requests and code search across your repos.", detail: "npx -y @modelcontextprotocol/server-github",
      removal: "copies", files: [`${HOME}/.claude.json`, `${LIB}/github/.mcp.json`],
    }),
    item({
      key: "server:linear", kind: "server", name: "Linear", description: "Issues, projects and cycles from your Linear workspace.", detail: "https://mcp.linear.app/mcp", url: "https://mcp.linear.app/mcp",
      scope: some("p-brigadier", "p-steady", "p-landing"), problem: { type: "needsSignIn", message: "Needs sign-in", serverId: "claude:user:linear", projectId: null },
    }),
    item({ key: "lib:playwright", kind: "server", name: "Playwright", description: "Drives a browser: clicks, types and takes screenshots.", detail: "npx @playwright/mcp@latest", scope: some("p-brigadier") }),
    item({ key: "lib:postgres", kind: "server", name: "postgres", detail: "uvx postgres-mcp postgresql://localhost/steady", on: false, scope: some("p-steady") }),
    item({
      key: "server:team-db", kind: "server", name: "team-db", description: "The team's staging database.", detail: "npx team-db-mcp", scope: some("p-steady"),
      locked: "Shared with your team in steady-app/.mcp.json: change it there.", removal: "none", files: [`${HOME}/Code/steady-app/.mcp.json`],
    }),
    item({ key: "lib:commit-messages", kind: "skill", name: "commit-messages", description: "Writes commit messages in this repo's style.", detail: `${LIB}/commit-messages/skills/commit-messages` }),
    item({
      key: "plugin:frontend-design", kind: "skill", name: "frontend-design", description: "Distinctive, production-grade web pages and components.", detail: `${HOME}/.claude/plugins/frontend-design`,
      problem: { type: "onlyIn", message: "Only in Claude Code: came in Claude Code's own plugin.", agents: ["claude"], reason: "ownPlugin" }, removal: "none",
    }),
    item({ key: "lib:pdf-forms", kind: "skill", name: "pdf-forms", description: "Reads PDF forms and fills them in.", detail: `${LIB}/pdf-forms/skills/pdf-forms`, scope: some("p-brigadier", "p-docs") }),
    item({
      key: "skill:pr-review", kind: "skill", name: "pr-review", description: "Reviews a pull request for bugs, tests and naming.", detail: `${HOME}/.claude/skills/pr-review`,
      problem: { type: "onlyIn", message: "Only in Claude Code", agents: ["claude"], reason: "removed" }, removal: "none", canShare: true, files: [`${HOME}/.claude/skills/pr-review/SKILL.md`],
    }),
    item({ key: "lib:release-notes", kind: "skill", name: "release-notes", description: "Drafts release notes from the pull requests merged since the last tag.", detail: `${LIB}/release-notes/skills/release-notes` }),
    item({ key: "lib:rust-style", kind: "skill", name: "rust-style", description: "This workspace's Rust rules: errors, naming and tests.", detail: `${LIB}/rust-style/skills/rust-style`, scope: some("p-brigadier") }),
    item({ key: "lib:tailwind-tokens", kind: "skill", name: "tailwind-tokens", detail: `${LIB}/tailwind-tokens/skills/tailwind-tokens`, on: false, scope: some("p-landing") }),
    item({ key: "lib:webapp-testing", kind: "skill", name: "webapp-testing", description: "Tests a local web app in a browser and reports what broke.", detail: `${LIB}/webapp-testing/skills/webapp-testing`, scope: some("p-brigadier", "p-steady", "p-landing") }),
  ];
}

const skillText = (name: string) => `---
name: ${name}
description: Drafts release notes from the pull requests merged since the last tag.
---

# Release notes

1. Find the last tag with \`git describe --tags --abbrev=0\`.
2. List the pull requests merged since then.
3. Group them under New, Fixed and Changed.
`;

let items: PluginItemView[] = startingItems();
let empty = false;
const calls: string[] = [];
/** SKILL.md files as saved, by path. */
const saved = new Map<string, string>();
let adding: { resolve: (results: PluginAgentResult[]) => void } | null = null;

const rank = (entry: PluginItemView) => (entry.problem ? 0 : 1);
/** The page as the daemon sends it: problems first, then A–Z. */
function view(): PluginsView {
  if (empty) return { projects, items: [], elsewhere: [] };
  return { projects, items: items.toSorted((a, b) => rank(a) - rank(b) || a.name.localeCompare(b.name, undefined, { sensitivity: "base" })), elsewhere: [] };
}

const ok = (): PluginAgentResult[] => AGENTS.map((agent) => ({ agent, ok: true, message: null }));
const listeners = new Set<() => void>();
const changed = () => listeners.forEach((listener) => listener());
const find = (key: string) => items.find((entry) => entry.key === key)!;
const edit = (key: string, change: Partial<PluginItemView>) => {
  items = items.map((entry) => (entry.key === key ? { ...entry, ...change } : entry));
  changed();
};

function details(key: string): PluginItemDetails {
  const shown = find(key);
  if (shown.kind === "server") {
    const server = shown.url
      ? { type: "http" as const, url: shown.url, headerNames: shown.name === "Context7" ? ["CONTEXT7_API_KEY"] : [] }
      : { type: "stdio" as const, command: shown.detail.split(" ")[0]!, args: shown.detail.split(" ").slice(1), cwd: null, envNames: shown.name === "GitHub" ? ["GITHUB_PERSONAL_ACCESS_TOKEN"] : [] };
    return { key, kind: "server", skills: [], server };
  }
  const path = shown.files[0]!;
  return {
    key,
    kind: "skill",
    skills: [{ name: shown.name, path, contents: saved.get(path) ?? skillText(shown.name), readOnly: shown.problem?.type === "onlyIn" && shown.problem.reason === "ownPlugin" ? "It came in Claude Code's own plugin: its next update would undo an edit." : null }],
    server: null,
  };
}

const adapter: PluginsAdapter = {
  read: async () => {
    reads++;
    return view();
  },
  subscribe: (onChange) => {
    listeners.add(onChange);
    return () => listeners.delete(onChange);
  },
  switchPlugin: async (key, on) => {
    calls.push(`switchPlugin ${key} ${on}`);
    edit(key, { on });
    return ok();
  },
  setScope: async (key, projectIds) => {
    calls.push(`setPluginScope ${key} ${projectIds ? projectIds.join(",") : "all"}`);
    edit(key, { scope: projectIds ? some(...projectIds) : { type: "allProjects" } });
    return ok();
  },
  share: async (key) => {
    calls.push(`sharePlugin ${key}`);
    edit(key, { problem: null, canShare: false, removal: "copies" });
    return ok();
  },
  checkAdd: async (text, name) => {
    calls.push(`checkPluginAdd ${text}${name ? ` name ${name}` : ""}`);
    if (!text.includes("sentry")) return { drafts: [{ name: "", kind: "server", detail: "", remote: false, signsIn: false, secrets: [], error: "That doesn't look like an MCP server or a skill." }], needsName: false };
    return { drafts: [{ name: "sentry", kind: "server", detail: "https://mcp.sentry.dev/mcp", remote: true, signsIn: true, secrets: [], error: null }], needsName: false };
  },
  add: (_text, name, projectIds, values) => {
    calls.push(`addPlugin ${name ?? "-"} ${projectIds ? projectIds.join(",") : "all"}${values ? ` values ${Object.keys(values).join(",")}` : ""}`);
    items.push(item({ key: "lib:sentry", kind: "server", name: "sentry", description: "Errors and slow pages from your Sentry projects.", detail: "https://mcp.sentry.dev/mcp", url: "https://mcp.sentry.dev/mcp", scope: projectIds ? some(...projectIds) : { type: "allProjects" } }));
    changed();
    return new Promise((resolve) => {
      adding = { resolve };
    });
  },
  remove: async (key) => {
    calls.push(`removePlugin ${key}`);
    if (deferredRemoval) await new Promise<void>((resolve) => { finishRemoval = resolve; });
    const removed = find(key);
    // An item an agent has of its own stays, only in that agent; anything else goes.
    if (removed.removal === "copies") {
      items = items.map((entry) => entry.key === key ? { ...entry, problem: { type: "onlyIn", message: "Only in Claude Code", agents: ["claude"], reason: "removed" }, canShare: true, removal: "none" } : entry);
    } else {
      items = items.filter((entry) => entry.key !== key);
    }
    // The deferred drive relies on completion to refresh, rather than a subscription event.
    if (!deferredRemoval) changed();
    return ok();
  },
  update: async (key) => {
    calls.push(`updatePlugin ${key}`);
    return ok();
  },
  details: async (key) => {
    calls.push(`getPluginItem ${key}`);
    return details(key);
  },
  saveSkill: async (key, path, contents, expected) => {
    calls.push(`savePluginSkill ${key} ${path}`);
    // Refused when the file changed since it was opened.
    if ((saved.get(path) ?? skillText(find(key).name)) !== expected) return "changed";
    saved.set(path, contents);
    return "saved";
  },
  signIn: async (key, serverId, projectId) => {
    calls.push(`signInMcpServer ${key} ${serverId} ${projectId ?? "all"}`);
    return new Promise(() => {});
  },
  cancelSignIn: async (key) => {
    calls.push(`cancelMcpSignIn ${key}`);
  },
  pickFolder: async () => {
    calls.push("pickFolder");
    return null;
  },
  trustProject: async (projectId) => {
    calls.push(`trustProject ${projectId}`);
  },
};
connectPlugins(adapter);

const pause = (ms = 60) => new Promise<void>((resolve) => setTimeout(resolve, ms));
/** Waits until `ready` says so, for at most `ms`. */
async function until(ready: () => boolean, ms = 10000) {
  for (const end = Date.now() + ms; !ready() && Date.now() < end;) await pause(50);
}
const all = <T extends Element = HTMLElement>(selector: string, root: ParentNode = document) => [...root.querySelectorAll<T>(selector)];
const text = (node: Element | null | undefined) => node?.textContent?.trim() ?? "";
const main = () => document.querySelector("main")!;
function click(label: string, root: ParentNode = document) {
  const button = all<HTMLButtonElement>("button, [role=switch], [role=tab], [role=radio]", root).find((node) => node.getAttribute("aria-label") === label || text(node) === label);
  if (!button) throw new Error(`No button: ${label}`);
  button.click();
}
/** Picks the radio row titled `title`. */
function choose(title: string) {
  const radio = all<HTMLButtonElement>("main [role=radio]").find((node) => text(node).startsWith(title));
  if (!radio) throw new Error(`No choice: ${title}`);
  radio.click();
}
/** Types `value` into the field named `label`. */
function type(label: string, value: string) {
  const field = all<HTMLTextAreaElement | HTMLInputElement>("textarea, input").find((node) => node.getAttribute("aria-label") === label);
  if (!field) throw new Error(`No field: ${label}`);
  const proto = field instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
  Object.getOwnPropertyDescriptor(proto, "value")!.set!.call(field, value);
  field.dispatchEvent(new Event("input", { bubbles: true }));
}
/** The switch of card `name`. */
function flip(name: string) {
  const card = all('[data-slot="plugin-row"]', main()).find((node) => text(node.querySelector('[data-slot="row-name"]')) === name);
  if (!card) throw new Error(`No card: ${name}`);
  card.querySelector<HTMLButtonElement>("[role=switch]")!.click();
}
const tab = (name: "MCP servers" | "Skills") => document.querySelector<HTMLButtonElement>(`main [role=tab][data-setting="${name}"]`)!.click();

function cards() {
  return all('[data-slot="plugin-row"]', main()).map((card) => {
    const line = card.querySelector('[data-slot="row-line"]');
    const toggle = card.querySelector("[role=switch]");
    return {
      name: text(card.querySelector('[data-slot="row-name"]')),
      line: text(line),
      tone: line?.getAttribute("data-tone") ?? null,
      chip: text(card.querySelector('[data-slot="scope-chip"]')) || null,
      mark: card.querySelector('[data-slot="plugin-mark"]')?.getAttribute("data-mark") ?? null,
      on: toggle?.getAttribute("aria-checked") === "true",
      locked: toggle?.hasAttribute("disabled") ?? false,
      dim: card.hasAttribute("data-off"),
    };
  });
}

/** Whether a field's placeholder shows whole. */
function fits(field: HTMLInputElement) {
  const style = getComputedStyle(field);
  const context = document.createElement("canvas").getContext("2d")!;
  context.font = `${style.fontWeight} ${style.fontSize} ${style.fontFamily}`;
  const room = field.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight);
  return context.measureText(field.placeholder).width <= room;
}

function read() {
  const column = document.querySelector('[data-slot="settings-page"] h1')?.closest(".mx-auto");
  const probe = document.body.appendChild(Object.assign(document.createElement("div"), { className: "max-w-settings" }));
  const settingsColumn = Boolean(column) && getComputedStyle(column as Element).maxWidth === getComputedStyle(probe).maxWidth;
  probe.remove();
  const dialog = document.querySelector<HTMLElement>("[data-dialog]");
  const grid = document.querySelector<HTMLElement>('[data-slot="plugins-grid"]');
  const search = document.querySelector<HTMLInputElement>('[data-slot="plugins-toolbar"] input');
  const editor = dialog?.querySelector('[data-slot="code-editor"]');
  return {
    settingsColumn,
    title: text(document.querySelector("main h1")),
    description: text(document.querySelector("main h1")?.nextElementSibling),
    tabs: all('main [role="tab"]').map((entry) => `${text(entry)}${entry.getAttribute("aria-selected") === "true" ? " *" : ""}`),
    toolbar: all('main [data-slot="plugins-toolbar"] button:not([role=tab])').map((button) => button.getAttribute("aria-label") ?? text(button)).filter(Boolean),
    search: search ? { placeholder: search.placeholder, label: search.getAttribute("aria-label"), fits: fits(search) } : null,
    columns: grid ? getComputedStyle(grid).gridTemplateColumns.split(" ").length : 0,
    cards: cards(),
    banners: all('main [data-slot="banner"]').map(text),
    radios: all('main [role="radiogroup"] [role="radio"]').map((radio) => `${text(radio)}${radio.getAttribute("aria-checked") === "true" ? " *" : ""}${radio.hasAttribute("disabled") ? " locked" : ""}`),
    projectSwitches: all('main [data-slot="project-switch"]').map((row) => {
      const toggle = row.querySelector("[role=switch]");
      return `${text(row)} ${toggle?.getAttribute("aria-checked")}${toggle?.hasAttribute("disabled") ? " locked" : ""}`;
    }),
    sectionLine: text(document.querySelector('main section[data-setting="Projects"] p')),
    advanced: all('main [data-slot="fold"]').map((fold) => ({ open: fold.querySelector("button")?.getAttribute("aria-expanded") === "true", text: text(fold) })),
    removeRow: text(document.querySelector('main [data-slot="remove-row"]')),
    empty: text(document.querySelector('main [data-slot="plugins-empty"]')),
    dialog: dialog
      ? {
          slot: dialog.getAttribute("data-dialog"),
          width: Math.round(dialog.getBoundingClientRect().width),
          viewport: innerWidth,
          rem: parseFloat(getComputedStyle(document.documentElement).fontSize),
          height: Math.round(dialog.getBoundingClientRect().height),
          title: text(dialog.querySelector("h2")),
          text: text(dialog),
          buttons: all("button", dialog).map((button) => button.getAttribute("aria-label") ?? text(button)).filter(Boolean),
          editable: !!dialog.querySelector('[data-slot="code-editor"] textarea'),
          lines: editor ? all("[data-line]", editor).length : 0,
          coloured: editor ? all("[data-line] span[style]", editor).length > 0 : false,
          status: text(dialog.querySelector("footer [role=status], footer [role=alert]")),
          banner: text(dialog.querySelector('[data-slot="banner"]')),
          body: text(dialog.querySelector('[data-slot="code-editor"]')),
        }
      : null,
    toasts: all('[data-slot="toast"]').map(text),
    alert: text(document.querySelector('main [role="alert"]')),
    overflow: main().scrollWidth > main().clientWidth,
  };
}

async function reset(setKey: (key: number) => void) {
  items = startingItems();
  empty = false;
  adding = null;
  saved.clear();
  document.querySelectorAll('[data-dialog] button[aria-label="Close"]').forEach((button) => (button as HTMLButtonElement).click());
  useApp.setState({ selection: { type: "settings", page: "plugins" } });
  setKey(Date.now());
  usePlugins.getState().refresh();
  await pause();
}

/** Opens each state from a fresh page. */
const OPENERS: Record<string, () => Promise<void>> = {
  L1: async () => {},
  L2: async () => tab("Skills"),
  L3: async () => {
    empty = true;
    usePlugins.getState().refresh();
  },
  D1: async () => click("Open Linear"),
  D2: async () => {
    tab("Skills");
    await pause();
    click("Open release-notes");
  },
  D3: async () => {
    tab("Skills");
    await pause();
    click("Open pr-review");
  },
  D4: async () => click("Open team-db"),
  F1: async () => {
    await OPENERS.D2!();
    await pause();
    click("Open");
  },
  F2: async () => {
    click("Open GitHub");
    await pause();
    click("Open");
    // Colours come once the highlighter has loaded.
    await until(() => !!document.querySelector('[data-slot="code-editor"] [data-line] span[style]'));
  },
  R1: async () => {
    click("Open GitHub");
    await pause();
    click("Remove GitHub");
  },
  A1: async () => click("Add"),
};

async function drive(setKey: (key: number) => void) {
  const seen: Record<string, unknown> = {};
  for (const state of ["L1", "L2", "L3", "D1", "D2", "D3", "D4", "F1", "F2", "R1"]) {
    await reset(setKey);
    await OPENERS[state]!();
    await pause(150);
    seen[state] = read();
  }

  // The card's switch: the whole item, in every agent.
  await reset(setKey);
  flip("Context7");
  await pause(120);
  seen.flipped = read();

  // A locked item's switch does nothing.
  flip("team-db");
  await pause(60);

  // Scope: one more project, then every project, then back to only some.
  await reset(setKey);
  await OPENERS.D1!();
  await pause();
  click("docs-site");
  await pause(120);
  seen.scoped = read();
  choose("All projects");
  await pause(120);
  seen.everywhere = read();
  choose("Only some projects");
  await pause(120);
  seen.someAgain = read();

  // Edit and save a SKILL.md; it opens again with the saved text.
  await reset(setKey);
  await OPENERS.F1!();
  await pause(120);
  type("release-notes's SKILL.md", `${skillText("release-notes")}4. Keep it under 300 words.\n`);
  await pause(60);
  seen.edited = read();
  // Escape doesn't close it while it has unsaved edits.
  document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  document.querySelector("[data-dialog]")?.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  await pause(60);
  seen.escaped = read();
  click("Save");
  await pause(150);
  seen.savedClosed = read();
  click("Open");
  await pause(150);
  seen.reopened = read();

  // Saved over a file that changed since it was opened: refused, said plainly, Reload shows it now.
  await reset(setKey);
  await OPENERS.F1!();
  await pause(120);
  type("release-notes's SKILL.md", `${skillText("release-notes")}4. Mine.\n`);
  saved.set(`${LIB}/release-notes/skills/release-notes/SKILL.md`, `${skillText("release-notes")}4. Theirs.\n`);
  click("Save");
  await pause(150);
  seen.refused = read();
  click("Reload");
  await pause(150);
  seen.reloaded = read();

  // Share an item only Claude Code has; Remove one an agent has of its own: it stays, only there.
  await reset(setKey);
  await OPENERS.D3!();
  await pause();
  click("Share with every agent");
  await pause(150);
  seen.shared = read();
  await reset(setKey);
  await OPENERS.R1!();
  await pause();
  click("Remove", document.querySelector("[data-dialog]")!);
  await pause(150);
  seen.removedCopies = read();

  // Add: every agent, all projects, its results after.
  await reset(setKey);
  click("Add");
  await pause();
  type("What to add", "claude mcp add --transport http sentry https://mcp.sentry.dev/mcp");
  click("Continue");
  await pause(120);
  seen.addReview = read();
  click("Add", document.querySelector("[data-dialog]")!);
  await pause(120);
  seen.adding = read();
  adding?.resolve(ok());
  await pause(120);
  seen.added = read();

  // Search narrows the grid.
  await reset(setKey);
  type("Search MCP servers", "git");
  await pause(120);
  seen.searched = read();
  type("Search MCP servers", "nothing like this");
  await pause(120);
  seen.noMatch = read();

  seen.calls = calls;
  return seen;
}

/** A removal can complete after dismissal, after leaving and returning, or while still live. */
const removalSnapshot = () => ({
  selection: useApp.getState().selection,
  history: useHistory.getState(),
  title: text(main().querySelector("h1")),
  dialog: text(document.querySelector("[data-dialog] h2")),
});

async function driveRemoval(setKey: (key: number) => void) {
  const seen: Record<string, unknown> = {};
  for (const scenario of ["cancel", "close-general", "close-back", "navigate-back", "active"] as const) {
    await reset(setKey);
    useHistory.setState({ entries: [], index: -1 });
    await OPENERS.D1!();
    await pause();
    if (scenario === "active") {
      // Successful active removal must replace detail without dropping this Forward entry.
      openSettings("general");
      await pause();
      stepHistory(-1);
      await pause();
    }
    click("Remove Linear");
    await pause();
    click("Remove", document.querySelector("[data-dialog]")!);
    await pause();
    if (!finishRemoval) throw new Error("Removal did not wait for its deferred answer");
    const pending = {
      cancel: all<HTMLButtonElement>("[data-dialog] button").find((node) => text(node) === "Cancel")?.disabled,
      close: document.querySelector<HTMLButtonElement>('[data-dialog] button[aria-label="Close"]')?.disabled,
    };
    if (scenario === "cancel") click("Cancel");
    if (scenario.startsWith("close-")) click("Close", document.querySelector("[data-dialog]")!);
    await pause();
    if (["close-general", "close-back", "navigate-back"].includes(scenario)) {
      openSettings("general");
      await pause();
      if (scenario.endsWith("back")) {
        stepHistory(-1);
        await pause();
      }
    }
    const before = removalSnapshot();
    const beforeReads = reads;
    finishRemoval();
    finishRemoval = null;
    await pause(150);
    seen[scenario] = {
      pending, before, after: removalSnapshot(), refreshed: reads > beforeReads,
      removed: !usePlugins.getState().view.items.some((entry) => entry.key === "server:linear"),
    };
  }
  return seen;
}

function Fixture() {
  const selection = useApp((state) => state.selection);
  const [key, setKey] = useState(0);
  const [result, setResult] = useState("");
  useEffect(() => {
    const done = (value: unknown) => setResult(JSON.stringify(value));
    const failed = (cause: unknown) => setResult(JSON.stringify({ error: String(cause) }));
    if (params.has("drive")) {
      void (deferredRemoval ? driveRemoval(setKey) : drive(setKey)).then(done, failed);
    } else {
      const state = params.get("state");
      if (state && OPENERS[state]) void reset(setKey).then(() => OPENERS[state]!()).then(() => pause(150)).then(() => done(read()), failed);
    }
  }, []);
  return (
    <TooltipProvider>
      <div className="bg-chrome text-foreground flex h-screen flex-col px-1 pb-1" style={width ? { width: `${width}px` } : undefined}>
        <SidebarProvider keepOpen className="relative min-h-0 flex-1 flex-col">
          <div className="relative flex min-h-0 min-w-0 flex-1">
            <SidebarPanel><SettingsNav /></SidebarPanel>
            <div className="relative flex min-w-0 flex-1 flex-col">
              <SettingsTitlebarStrip><TerminalButton /></SettingsTitlebarStrip>
              <main className="bg-background border-divider rounded-e-page min-h-0 flex-1 overflow-hidden border-l">
                {deferredRemoval && selection.type === "settings" && selection.page === "general"
                  ? <GeneralPage /> : <PluginsPage key={key} />}
              </main>
              <Toaster className="top-titlebar pt-2" />
            </div>
          </div>
          <TitlebarNav />
        </SidebarProvider>
      </div>
      {result && <pre id="plugins-result">{result}</pre>}
    </TooltipProvider>
  );
}
createRoot(document.getElementById("root")!).render(<Fixture />);
