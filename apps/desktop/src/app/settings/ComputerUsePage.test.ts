import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";
import ReactDOMServer from "react-dom/server";
import { createServer } from "vite";

import type { ComputerAccess } from "@/ipc/generated";

test("computer use shows each missing permission with one Allow button, and nothing where unavailable", async () => {
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { ComputerUseBody } = await server.ssrLoadModule("/src/app/settings/ComputerUsePage.tsx");
    const render = (access: ComputerAccess | null, asked = false) =>
      ReactDOMServer.renderToStaticMarkup(
        React.createElement(ComputerUseBody, { access, asked, onAllow: async () => {} }),
      );
    const none: ComputerAccess = { available: true, accessibility: false, screenRecording: false, restarting: false, problem: null };

    const html = render(none);
    assert.match(html, />Control apps</);
    assert.match(html, />See the screen</);
    assert.equal([...html.matchAll(/Not allowed yet/g)].length, 2);
    const buttons = [...html.matchAll(/<button[^>]*>([^<]*)<\/button>/g)].map((match) => match[1]);
    assert.deepEqual(buttons, ["Allow…", "Allow…"]);
    assert.doesNotMatch(html, /turn on Brigadier Computer Use/);

    const half = render({ ...none, accessibility: true, problem: "Couldn't read." }, true);
    assert.match(half, /Allowed/);
    assert.equal([...half.matchAll(/>Allow…</g)].length, 1);
    // After an Allow, the missing one also offers Start over; before it, only Allow….
    const halfButtons = [...half.matchAll(/<button[^>]*>([^<]*)<\/button>/g)].map((match) => match[1]);
    assert.deepEqual(halfButtons, ["Start over", "Allow…"]);
    assert.match(half, /Device Control and Data Access \(called Accessibility before macOS 27\)/);
    assert.match(half, /Turn on Brigadier Computer Use there; this page updates by itself\./);
    assert.match(half, /Couldn(&#x27;|')t read\./);
    assert.match(half, /Press Start over: Brigadier Computer Use forgets it, and macOS asks again\./);
    assert.doesNotMatch(half, /restarting/);

    // Both on: no steps, even after an Allow; a restart for the screen grant is said plainly.
    const both = render({ ...none, accessibility: true, screenRecording: true, restarting: true }, true);
    assert.equal([...both.matchAll(/>Allow…</g)].length, 0);
    assert.doesNotMatch(both, /Turn on Brigadier Computer Use/);
    assert.match(both, /Brigadier Computer Use is restarting to use the permission you just gave\./);

    assert.equal(render({ ...none, available: false }), "");
    assert.equal(render(null), "");
  } finally {
    await server.close();
  }
});
