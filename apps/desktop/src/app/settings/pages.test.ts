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

// Configurations holds what the Configuration, Conversations and Personalization pages did:
// search still finds every one of their settings there, and the old pages' names find the page.
test("search finds every merged setting on Configurations, and no Recent activity", async () => {
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
        `"${query}" finds a row on Configurations`,
      );
    }
    // A level's row brings the default level's row into view.
    const level = found("Approve for me").find((result) => result.row?.label === "Approve for me");
    assert.equal(level?.row?.target, "Default permission level");
    for (const name of ["Configurations", "Conversations", "Personalization"]) {
      assert.ok(found(name).some((result) => result.page.id === "configuration" && result.row === null), name);
    }
    assert.deepEqual(found("Recent activity"), []);
  } finally {
    await server.close();
  }
});

// Links to the merged pages open Configurations, at the setting each one held.
test("the merged pages' ids open Configurations at their setting", async () => {
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
