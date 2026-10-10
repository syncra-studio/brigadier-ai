import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";
import ReactDOMServer from "react-dom/server";
import { createServer } from "vite";

import benchRun from "@/fixtures/boards/computer-bench-run.json" with { type: "json" };
import type { ComputerAccess, ComputerAction } from "@/ipc/generated";

const actions = benchRun as ComputerAction[];
const steps = (html: string) => [...html.matchAll(/data-slot="computer-step"/g)].length;
const details = (html: string) => [...html.matchAll(/data-slot="computer-step-detail"[^>]*>([^<]*)</g)].map((match) => match[1]);
const buttons = (html: string) => [...html.matchAll(/<button[^>]*>([^<]*)<\/button>/g)].map((match) => match[1]);
const labels = (html: string) => [...html.matchAll(/<button[^>]*aria-label="([^"]*)"/g)].map((match) => match[1]);

test("the timeline is one disclosure: action and outcome on every step, route and time only on the shown step", async () => {
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { ComputerTimelineView } = await server.ssrLoadModule("/src/app/conversation/ComputerTimeline.tsx");
    const render = (props: Record<string, unknown>) =>
      ReactDOMServer.renderToStaticMarkup(
        React.createElement(ComputerTimelineView, { actions, live: false, looks: 0, earlier: false, onEarlier: () => {}, ...props }),
      );

    const closed = render({});
    assert.equal([...closed.matchAll(/data-slot="computer-timeline"/g)].length, 1);
    assert.match(closed, /Used the computer · 9 steps in target-range/);
    assert.equal(steps(closed), 0);

    const open = render({ defaultOpen: true });
    assert.equal(steps(open), actions.length);
    // The newest step is shown first: the foreground click, the only one with details.
    assert.deepEqual(details(open), [`Brought the window to the front while you were away · target-range, “Minimised Target” · ${Math.round(actions.at(-1)!.dispatchMs)} ms`]);
    assert.match(open, /Step 9 of 9/);
    assert.deepEqual(labels(open).filter((label) => label !== "Open the screenshot full size"), ["Previous step", "Play", "Next step"]);
    assert.match(open, /aria-label="Next step" aria-disabled="true"/);
    assert.match(open, /role="toolbar"/);
    assert.match(open, /aria-current="step"/);
    // Outcomes in words, a failure marked as one.
    assert.match(open, /Clicked “Check 8 pt”<\/span><span aria-hidden="true"[^>]*>·<\/span><span class="[^"]*">worked</);
    assert.match(open, /class="[^"]*text-destructive[^"]*">couldn(&#x27;|')t do it in the background</);
    assert.match(open, /data-slot="computer-shot"/);

    const live = render({ live: true, actions: actions.slice(0, 2) });
    assert.match(live, /Using the computer · Clicked “Check 8 pt” in target-range/);
    assert.match(live, /shimmer/);

    const looked = render({ actions: [], looks: 3 });
    assert.match(looked, /Used the computer · looked 3 times/);
    assert.doesNotMatch(looked, /computer-steps/);

    assert.match(render({ defaultOpen: true, earlier: true }), />Load earlier</);
  } finally {
    await server.close();
  }
});

test("the permission item has one Allow per missing grant and closes its ask once both are in", async () => {
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { ComputerAccessBody } = await server.ssrLoadModule("/src/app/conversation/ComputerAccessRow.tsx");
    const render = (access: ComputerAccess | null, asked = false) =>
      ReactDOMServer.renderToStaticMarkup(
        React.createElement(ComputerAccessBody, { what: "Workers need permission to use apps", access, asked, onAllow: async () => {} }),
      );
    const none: ComputerAccess = { available: true, accessibility: false, screenRecording: false, restarting: false, problem: null };

    const both = render(none);
    assert.match(both, /Workers need permission to use apps/);
    assert.match(both, />Control apps</);
    assert.match(both, />See the screen</);
    assert.deepEqual(buttons(both), ["Allow…", "Allow…"]);
    assert.doesNotMatch(both, /System Settings/);

    const half = render({ ...none, screenRecording: true }, true);
    assert.deepEqual(buttons(half), ["Allow…"]);
    assert.match(half, />Control apps</);
    assert.doesNotMatch(half, />See the screen</);
    assert.match(half, /Device Control and Data Access \(called Accessibility before macOS 27\)/);
    assert.match(half, /Turn on Brigadier Computer Use there; this page updates by itself\./);

    const done = render({ ...none, accessibility: true, screenRecording: true }, true);
    assert.deepEqual(buttons(done), []);
    assert.match(done, /Allowed\. Workers can use apps now\./);
    assert.doesNotMatch(done, /System Settings/);
  } finally {
    await server.close();
  }
});
