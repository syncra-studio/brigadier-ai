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
