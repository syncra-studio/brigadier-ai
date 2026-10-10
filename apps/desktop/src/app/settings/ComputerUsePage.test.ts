import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";
import ReactDOMServer from "react-dom/server";
import { createServer } from "vite";

import type { ComputerAccess, ComputerGrant } from "@/ipc/generated";

// Each button's text (an icon button's is its hidden name); the footnote's tip is last, named by aria-label.
const buttons = (html: string) =>
  [...html.matchAll(/<button[^>]*>(.*?)<\/button>/g)].map((match) => (match[1] ?? "").replace(/<[^>]+>/g, "").trim());
const count = (html: string, pattern: RegExp) => [...html.matchAll(pattern)].length;

test("computer use shows a status, each permission's state and one action, Start over only after an Allow, and nothing where unavailable", async () => {
  // The tips read their offset from the theme; a server render has none.
  const globals = globalThis as Record<string, unknown>;
  const saved = { document: globals.document, getComputedStyle: globals.getComputedStyle };
  globals.document = { documentElement: {} };
  globals.getComputedStyle = () => ({ getPropertyValue: () => "" });
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { ComputerUseBody } = await server.ssrLoadModule("/src/app/settings/ComputerUsePage.tsx");
    const render = (access: ComputerAccess | null, asked: ComputerGrant[] = []) =>
      ReactDOMServer.renderToStaticMarkup(
        React.createElement(ComputerUseBody, {
          access,
          asked: new Set(asked),
          onAllow: async () => {},
          onOpen: async () => {},
          onRefresh: async () => {},
        }),
      );
    const none: ComputerAccess = { available: true, accessibility: false, screenRecording: false, restarting: false, problem: null };

    // Nothing allowed yet: the status says how many are needed; each row offers only Allow….
    const html = render(none);
    assert.match(html, />Finish setup</);
    assert.match(html, />2 permissions needed</);
    assert.match(html, /role="status"/);
    assert.match(html, />Control apps</);
    assert.match(html, />See the screen</);
    assert.match(html, /Lets workers click, type and read app windows\./);
    assert.match(html, /Lets workers look at app windows to check their work\./);
    assert.equal(count(html, /Not allowed</g), 2);
    assert.deepEqual(buttons(html), ["Check again", "Allow…", "Allow…", ""]);
    assert.match(html, /aria-label="Where to find it"/);
    assert.match(html, /a separate helper, <span[^>]*>Brigadier Computer Use<\/span>/);
    assert.doesNotMatch(html, /Start over/);
    // The System Settings list names are on the tip, not the page.
    assert.doesNotMatch(html, /Device Control and Data Access/);

    // One allowed, the other asked for: Open System Settings on both, Start over under the rows.
    const half = render({ ...none, accessibility: true }, ["screenRecording"]);
    assert.match(half, />1 permission needed</);
    assert.equal(count(half, />Allowed</g), 1);
    assert.equal(count(half, /Not allowed</g), 1);
    assert.deepEqual(buttons(half), ["Check again", "Open System Settings", "Open System Settings", "Allow…", "Start over", ""]);
    assert.match(half, /Turned it on, but it still says Not allowed\?/);
    assert.match(half, /Brigadier Computer Use forgets its old entry, and macOS asks again\./);

    // Both asked for and missing: Start over names each.
    const both = render(none, ["accessibility", "screenRecording"]);
    assert.deepEqual(buttons(both).filter((text) => text.startsWith("Start over")), [
      "Start over for Control apps",
      "Start over for See the screen",
    ]);

    // Both allowed: Ready, no Allow and no Start over, even after an Allow.
    const ready = render({ ...none, accessibility: true, screenRecording: true }, ["screenRecording"]);
    assert.match(ready, />Ready</);
    assert.match(ready, /Workers can see and use apps on this Mac\./);
    assert.equal(count(ready, />Allowed</g), 2);
    assert.deepEqual(buttons(ready), ["Check again", "Open System Settings", "Open System Settings", ""]);
    assert.doesNotMatch(ready, /Not allowed|Start over/);

    // A restart for the screen grant is said plainly in the status.
    const restarting = render({ ...none, accessibility: true, screenRecording: true, restarting: true });
    assert.match(restarting, />Almost ready</);
    assert.match(restarting, /Brigadier Computer Use is restarting to use the permission you just gave\./);

    // A failed read says why, and offers no Start over (nothing is known to be missing).
    const failed = render({ ...none, problem: "Couldn't read." }, ["accessibility"]);
    assert.match(failed, />Couldn(&#x27;|')t check permissions</);
    assert.match(failed, /Couldn(&#x27;|')t read\./);
    assert.doesNotMatch(failed, /Start over|Not allowed/);
    assert.equal(count(failed, /Not checked</g), 2);

    assert.equal(render({ ...none, available: false }), "");
    assert.equal(render(null), "");
  } finally {
    await server.close();
    Object.assign(globals, saved);
  }
});
