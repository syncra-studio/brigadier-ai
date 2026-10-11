import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";
import ReactDOMServer from "react-dom/server";
import { createServer } from "vite";

import type { AppInfo } from "@/ipc/generated";

// Reading computer use's permissions starts its helper, so Settings must know whether to list
// the page without one: the launch smoke check opens the Inspector, a Settings page, and a
// helper started then took the CPU its timings measure.
test("Settings lists computer use on macOS from the app's info, without reading its permissions", async () => {
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

    // No permission read has answered in this process, and none is needed.
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
      "Short replies",
      "Show context window usage",
      "Show the Full access notice",
      "Use spare quota to deepen the Brain",
      "Memories",
      "Default permission level",
      "Ask for approval",
      "Approve for me",
      "Full access",
      "Review the work",
    ]) {
      assert.ok(
        found(query).some((result) => result.page.id === "configuration" && result.row !== null),
        `"${query}" finds a row on Configuration`,
      );
    }
    // A level's row brings the default level's row into view.
    const level = found("Approve for me").find((result) => result.row?.label === "Approve for me");
    assert.equal(level?.row?.target, "Default permission level");
    // Its own name, and the names it went by.
    for (const name of ["Configuration", "Configurations", "Conversations", "Personalization"]) {
      assert.ok(found(name).some((result) => result.page.id === "configuration" && result.row === null), name);
    }
    assert.deepEqual(found("Recent activity"), []);
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
    for (const [id, label] of [
      ["conversations", "Model for sessions"],
      ["personalization", "Memories"],
    ]) {
      openSettings(id);
      assert.deepEqual(useApp.getState().selection, { type: "settings", page: "configuration" });
      assert.equal(useReveal.getState().label, label);
    }
  } finally {
    await server.close();
    Object.assign(globals, saved);
  }
});

// Providers holds what the Providers, Accounts and Routing pages did: search finds every one of
// their rows there, a row whose place moved brings its replacement into view, and the old
// pages' names find the page.
test("search finds every row of Providers, Accounts and Routing on Providers", async () => {
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { searchSettings } = await server.ssrLoadModule("/src/app/settings/pages.ts");
    type Result = { page: { id: string }; row: { label: string; target?: string } | null };
    const found = (query: string): Result[] => searchSettings(query);
    const onProviders = (query: string) =>
      found(query).find((result) => result.page.id === "providers" && result.row !== null);
    for (const query of [
      // Providers
      "Agents",
      "Available models",
      "Advanced",
      "Run the first-run setup again",
      // Accounts
      "Switch to another account when one runs out",
      "Add account",
      "This computer's login",
      // Routing
      "Models Brigadier can give work to",
      "Refresh rankings",
      "Who does what",
      "Kinds of work in detail",
      "Rules",
      "Model details",
      "Model ratings",
      "Model registry",
      // New
      "Use this model",
      "Don't give work",
      "Installed agents",
    ]) {
      assert.ok(onProviders(query), `"${query}" finds a row on Providers`);
    }
    // Rows that moved bring their replacement into view.
    const target = (label: string) =>
      found(label).find((result) => result.row?.label === label)?.row?.target;
    assert.equal(target("Models Brigadier can give work to"), "Rules");
    assert.equal(target("Agents"), "Claude Code");
    assert.equal(target("Add account"), "Claude Code");
    assert.equal(target("Use this model"), "Models");
    assert.equal(target("Advanced"), "Installed agents");
    assert.equal(target("Refresh rankings"), "Model ratings");
    for (const name of ["Providers", "Accounts", "Routing"]) {
      assert.ok(found(name).some((result) => result.page.id === "providers" && result.row === null), name);
    }
    assert.ok(!found("Accounts").some((result) => result.page.id === ("accounts" as string)));
  } finally {
    await server.close();
  }
});

// Links to Accounts and Routing open Providers at the setting each one held.
test("the old Accounts and Routing ids open Providers at their setting", async () => {
  const globals = globalThis as Record<string, unknown>;
  const saved = { requestAnimationFrame: globals.requestAnimationFrame };
  globals.requestAnimationFrame = () => 0;
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { useApp } = await server.ssrLoadModule("/src/state/store.ts");
    const { openSettings } = await server.ssrLoadModule("/src/state/actions.ts");
    const { useReveal } = await server.ssrLoadModule("/src/app/settings/reveal.ts");
    for (const [id, label] of [
      ["accounts", "Switch to another account when one runs out"],
      ["routing", "Who does what"],
    ]) {
      openSettings(id);
      assert.deepEqual(useApp.getState().selection, { type: "settings", page: "providers" });
      assert.equal(useReveal.getState().label, label);
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
      "Personal: Keyboard shortcuts",
      "Agents: Providers",
      "Agents: Usage",
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
      { title: "Personal", pages: ["General", "Voice", "Configuration", "Keyboard shortcuts"] },
      { title: "Agents", pages: ["Providers", "Usage", "Git", "Computer use"] },
      { title: "Browser &amp; Data", pages: ["Browser", "Import Browser Data"] },
      { title: "System", pages: ["Storage", "Inspector"] },
      { title: null, pages: ["Archived chats"] },
    ]);
    type Result = { page: Page; row: unknown };
    const first = (query: string) => (searchSettings(query) as Result[])[0];
    for (const [query, id, group] of [
      ["configurations", "configuration", "Personal"],
      ["configuration", "configuration", "Personal"],
      ["usage", "usage", "Agents"],
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
