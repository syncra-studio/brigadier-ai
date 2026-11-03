import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import React from "react";
import ReactDOMServer from "react-dom/server";
import { createServer, type ViteDevServer } from "vite";

import type * as Steps from "@/app/settings/routing/StepsList";
import type * as Rankings from "@/fixtures/rankings";
import type { GuidanceRow, ModelRules } from "@/ipc/generated";

let server: ViteDevServer;
let StepsList: typeof Steps.StepsList;
let IDLE_RESEARCH: typeof Rankings.IDLE_RESEARCH;
before(async () => {
  const globals = globalThis as Record<string, unknown>;
  globals.document ??= { documentElement: {} };
  globals.getComputedStyle ??= () => ({ getPropertyValue: () => "" });
  server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  ({ StepsList } = await server.ssrLoadModule("/src/app/settings/routing/StepsList.tsx") as typeof Steps);
  ({ IDLE_RESEARCH } = await server.ssrLoadModule("/src/fixtures/rankings.ts") as typeof Rankings);
});
after(() => server.close());

const FIXED = "For example, if your rules keep the first model for Computer use out, the next model in its order runs; with none left, the worker is refused.";
const rows = (work: string): GuidanceRow[] => [
  { work, agent: "claude", model: "blocked", effort: null, reason: "" },
  { work, agent: "codex", model: "allowed", effort: null, reason: "" },
];
const example = (shipped: GuidanceRow[], mine: GuidanceRow[] = []) => {
  const rules: ModelRules = { path: "", shipped, rows: mine, rules: [{ type: "forbid", names: ["blocked"] }], suggestions: [], research: IDLE_RESEARCH };
  const markup = ReactDOMServer.renderToStaticMarkup(React.createElement(StepsList, { rules, groups: [] }));
  return /<p data-slot="routing-steps-example"[^>]*>(.*?)<\/p>/.exec(markup)?.[1];
};

test("the fixed picking example uses Computer use", () => {
  assert.equal(example([]), FIXED);
});

test("role rows never become fallback examples, including Fusion's underlying work names", () => {
  for (const work of ["Adversarial review", "Fusion", "fusion-claude", "fusion-codex", "Fallback"]) {
    assert.equal(example(rows(work)), FIXED, work);
    assert.equal(example([], rows(work)), FIXED, `${work}: My order`);
  }
});

test("generated examples still use the first eligible fallback order after role rows", () => {
  const roles = ["Adversarial review", "fusion-claude", "fusion-codex", "Fallback"].flatMap(rows);
  assert.equal(example([...roles, ...rows("Computer use")]), "For example, Computer use: your rules keep Blocked out, so Allowed, next in the automatic advice, runs.");
  assert.equal(example(roles, rows("Docs")), "For example, Docs: your rules keep Blocked out, so Allowed, next in your order, runs.");
});
