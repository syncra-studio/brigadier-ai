import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";
import ReactDOMServer from "react-dom/server";
import { createServer } from "vite";

import type { AppInfo } from "@/ipc/generated";

// Settings lists the Computer use page on macOS only, from the app's own info.
test("Settings lists computer use on macOS from the app's info", async () => {
  const globals = globalThis as Record<string, unknown>;
  const saved = { document: globals.document, getComputedStyle: globals.getComputedStyle };
  globals.document = { documentElement: {} };
  globals.getComputedStyle = () => ({ getPropertyValue: () => "" });
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { useApp } = await server.ssrLoadModule("/src/state/store.ts");
    const { shownSettingsPages } = await server.ssrLoadModule("/src/app/settings/pages.ts");
    const { SettingsNav } = await server.ssrLoadModule("/src/app/settings/SettingsNav.tsx");
    const on = (platform: string) => {
      useApp.setState({ info: { version: "0", platform } as AppInfo });
      return shownSettingsPages().some((page: { id: string }) => page.id === "computerUse");
    };

    assert.equal(on("macos"), true);
    assert.match(ReactDOMServer.renderToStaticMarkup(React.createElement(SettingsNav)), />Computer use</);
    assert.equal(on("linux"), false);
    assert.equal(on("windows"), false);
    assert.doesNotMatch(ReactDOMServer.renderToStaticMarkup(React.createElement(SettingsNav)), />Computer use</);
  } finally {
    await server.close();
    Object.assign(globals, saved);
  }
});

// Configuration holds what the Configuration, Conversations and Personalization pages did:
// search still finds every one of their settings there, and the old pages' names find the page.
test("search finds every merged setting on Configuration, and no Recent activity", async () => {
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { searchSettings } = await server.ssrLoadModule("/src/app/settings/pages.ts");
    type Result = { page: { id: string }; row: { label: string; target?: string } | null };
    const found = (query: string): Result[] => searchSettings(query);
    for (const query of [
      "Orchestrator",
      "Chat model",
      "Model for sessions",
      "Start workers on its own",
      "Show the Full access notice",
      "Default permission level",
      "Ask",
      "Accept edits",
      "Full access",
    ]) {
      assert.ok(
        found(query).some((result) => result.page.id === "configuration" && result.row !== null),
        `"${query}" finds a row on Configuration`,
      );
    }
    // The memories have a page of their own, under Personal.
    assert.ok(found("Memories").some((result) => result.page.id === "memory" && result.row?.label === "Memories"));
    // A level's row brings the default level's row into view.
    const level = found("Accept edits").find((result) => result.row?.label === "Accept edits");
    assert.equal(level?.row?.target, "Default permission level");
    // Its own name, and the names it went by.
    for (const name of ["Configuration", "Configurations", "Conversations", "Personalization"]) {
      assert.ok(found(name).some((result) => result.page.id === "configuration" && result.row === null), name);
    }
    assert.deepEqual(found("Recent activity"), []);
    // What the composer shows and how it sends are General's.
    for (const query of ["Show context window usage", "Follow-up behaviour", "Plain text composer"]) {
      assert.ok(
        found(query).some((result) => result.page.id === "general" && result.row?.label === query),
        `"${query}" finds its row on General`,
      );
    }
    // Enter always sends: there is no Send shortcut to find.
    assert.ok(!found("Send shortcut").some((result) => result.row?.label === "Send shortcut"));
  } finally {
    await server.close();
  }
});

// Links to the merged pages open Configuration, at the setting each one held.
test("the merged pages' ids open Configuration at their setting", async () => {
  const globals = globalThis as Record<string, unknown>;
  const saved = { requestAnimationFrame: globals.requestAnimationFrame };
  globals.requestAnimationFrame = () => 0;
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { useApp } = await server.ssrLoadModule("/src/state/store.ts");
    const { openSettings } = await server.ssrLoadModule("/src/state/actions.ts");
    const { useReveal } = await server.ssrLoadModule("/src/app/settings/reveal.ts");
    for (const [id, label, page] of [
      ["conversations", "Model for sessions", "configuration"],
      ["personalization", "Memories", "memory"],
    ]) {
      openSettings(id);
      assert.deepEqual(useApp.getState().selection, { type: "settings", page });
      assert.equal(useReveal.getState().label, label);
    }
  } finally {
    await server.close();
    Object.assign(globals, saved);
  }
});

// Providers holds what the Providers, Accounts and Usage pages did: search finds every row of it
// there, a row whose place moved brings its replacement into view, and the old pages' names find
// the page that took them.
test("search finds every row of Providers on its page", async () => {
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { searchSettings } = await server.ssrLoadModule("/src/app/settings/pages.ts");
    const { PROVIDERS_ROWS } = await server.ssrLoadModule("/src/app/settings/providers/ProvidersPage.tsx");
    type Row = { label: string; target?: string };
    type Result = { page: { id: string }; row: Row | null };
    const found = (query: string): Result[] => searchSettings(query);
    const on = (page: string, label: string) =>
      found(label).find((result) => result.page.id === page && result.row?.label === label);
    for (const { label } of Object.values(PROVIDERS_ROWS) as Row[]) {
      assert.ok(on("providers", label), `"${label}" finds a row on providers`);
    }
    // Rows that moved bring their replacement into view.
    assert.equal(on("providers", "Usage")?.row?.target, "Accounts");
    assert.equal(on("providers", "Add account")?.row?.target, "Accounts");
    for (const [name, page] of [["Providers", "providers"], ["Accounts", "providers"], ["Usage", "providers"]] as const) {
      assert.ok(found(name).some((result) => result.page.id === page && result.row === null), name);
    }
    assert.ok(!found("Usage").some((result) => result.page.id === ("usage" as string)));
  } finally {
    await server.close();
  }
});

// Links to Accounts and Usage open Providers at the accounts.
test("the old Accounts and Usage ids open Providers at the accounts", async () => {
  const globals = globalThis as Record<string, unknown>;
  const saved = { requestAnimationFrame: globals.requestAnimationFrame };
  globals.requestAnimationFrame = () => 0;
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { useApp } = await server.ssrLoadModule("/src/state/store.ts");
    const { openSettings } = await server.ssrLoadModule("/src/state/actions.ts");
    const { useReveal } = await server.ssrLoadModule("/src/app/settings/reveal.ts");
    for (const id of ["accounts", "usage"]) {
      openSettings(id);
      assert.deepEqual(useApp.getState().selection, { type: "settings", page: "providers" });
      assert.equal(useReveal.getState().label, "Accounts");
    }
  } finally {
    await server.close();
    Object.assign(globals, saved);
  }
});

// Every Browser row is found on its page, and Import is no longer one of them: it has a page of
// its own, which "import" finds first, on macOS where importing exists.
test("search finds every Browser row; import finds Import Browser Data", async () => {
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { useApp } = await server.ssrLoadModule("/src/state/store.ts");
    const { searchSettings } = await server.ssrLoadModule("/src/app/settings/pages.ts");
    const { BROWSER_ROWS } = await server.ssrLoadModule("/src/app/settings/BrowserPage.tsx");
    type Result = { page: { id: string }; row: { label: string; target?: string } | null };
    const found = (query: string): Result[] => searchSettings(query);
    const onBrowser = (label: string) =>
      found(label).find((result) => result.page.id === "browser" && result.row?.label === label);
    for (const { label } of Object.values(BROWSER_ROWS) as { label: string }[]) {
      assert.ok(onBrowser(label), `"${label}" finds a row on Browser`);
    }
    useApp.setState({ info: { version: "0", platform: "macos" } as AppInfo });
    assert.equal(onBrowser("Import from your browser"), undefined);
    const imports = found("import");
    assert.deepEqual(imports[0], { page: imports[0]?.page, row: null });
    assert.equal(imports[0]?.page.id, "browserImport");
    assert.ok(imports.every((result) => result.page.id === "browserImport"), "every import result is on its page");
    assert.ok(
      found("Import from your browser").some((result) => result.page.id === "browserImport" && result.row?.label === "Import from your browser"),
    );
    // Elsewhere there is no importing, and no page for it.
    useApp.setState({ info: { version: "0", platform: "linux" } as AppInfo });
    assert.deepEqual(found("import"), []);
  } finally {
    await server.close();
  }
});

// The navigation: Personal, Agents, Browser & Data and System, then Archived chats on its own,
// set apart without a title. Searching a page's name, old or new, finds it in its new place.
test("Settings lists its pages in their groups, Archived chats last and untitled", async () => {
  const globals = globalThis as Record<string, unknown>;
  const saved = { document: globals.document, getComputedStyle: globals.getComputedStyle };
  globals.document = { documentElement: {} };
  globals.getComputedStyle = () => ({ getPropertyValue: () => "" });
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { useApp } = await server.ssrLoadModule("/src/state/store.ts");
    const { searchSettings, shownSettingsPages } = await server.ssrLoadModule("/src/app/settings/pages.ts");
    const { SettingsNav } = await server.ssrLoadModule("/src/app/settings/SettingsNav.tsx");
    useApp.setState({ info: { version: "0", platform: "macos" } as AppInfo });
    type Page = { id: string; label: string; group: string };
    const grouped = (shownSettingsPages() as Page[]).map((page) => `${page.group}: ${page.label}`);
    assert.deepEqual(grouped, [
      "Personal: General",
      "Personal: Voice",
      "Personal: Configuration",
      "Personal: Memory",
      "Personal: Keyboard shortcuts",
      "Agents: Providers",
      "Agents: Plugins",
      "Agents: Git",
      "Agents: Computer use",
      "Browser & Data: Browser",
      "Browser & Data: Import Browser Data",
      "System: Storage",
      "System: Inspector",
      "end: Archived chats",
    ]);
    const markup = ReactDOMServer.renderToStaticMarkup(React.createElement(SettingsNav));
    const sections = [...markup.matchAll(/<section[^>]*>([\s\S]*?)<\/section>/g)].map((match) => ({
      title: /<h2[^>]*>([^<]*)<\/h2>/.exec(match[1] ?? "")?.[1] ?? null,
      pages: [...(match[1] ?? "").matchAll(/<span class="truncate">([^<]*)<\/span>/g)].map((page) => page[1]),
    }));
    assert.deepEqual(sections, [
      { title: "Personal", pages: ["General", "Voice", "Configuration", "Memory", "Keyboard shortcuts"] },
      { title: "Agents", pages: ["Providers", "Plugins", "Git", "Computer use"] },
      { title: "Browser &amp; Data", pages: ["Browser", "Import Browser Data"] },
      { title: "System", pages: ["Storage", "Inspector"] },
      { title: null, pages: ["Archived chats"] },
    ]);
    type Result = { page: Page; row: unknown };
    for (const label of ["MCP servers", "Skills"]) {
      const result = (searchSettings(label) as { page: Page; row: { label: string } | null }[])[0];
      assert.equal(result?.page.id, "plugins");
      assert.equal(result?.row?.label, label);
    }
    const first = (query: string) => (searchSettings(query) as Result[])[0];
    for (const [query, id, group] of [
      ["configurations", "configuration", "Personal"],
      ["configuration", "configuration", "Personal"],
      ["plugins", "plugins", "Agents"],
      ["usage", "providers", "Agents"],
      ["storage", "storage", "System"],
      ["import", "browserImport", "Browser & Data"],
      ["archived", "archived", "end"],
    ] as const) {
      const found = first(query);
      assert.deepEqual([found?.page.id, found?.page.group, found?.row], [id, group, null], query);
    }
  } finally {
    await server.close();
    Object.assign(globals, saved);
  }
});
