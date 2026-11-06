import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { createServer, type ViteDevServer } from "vite";

import type * as Row from "@/app/settings/routing/ResearchRow";
import { IDLE_RESEARCH } from "@/fixtures/rankings";
import type { RankingsRefresh } from "@/ipc/generated";

// The line needs the model names lib/routing reads through the app's modules: loaded by Vite.
let server: ViteDevServer;
let researchLine: typeof Row.researchLine;
before(async () => {
  const globals = globalThis as Record<string, unknown>;
  globals.document ??= { documentElement: {} };
  globals.getComputedStyle ??= () => ({ getPropertyValue: () => "" });
  server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  ({ researchLine } = (await server.ssrLoadModule("/src/app/settings/routing/ResearchRow.tsx")) as typeof Row);
});
after(() => server.close());

const HOUR = 3_600_000;
const ran = (researcherUnrated: boolean, state: RankingsRefresh["state"] = "done"): RankingsRefresh => ({
  ...IDLE_RESEARCH,
  state,
  startedAtMs: 1,
  finishedAtMs: 1,
  model: "opencode/kimi",
  researcherUnrated,
  changes: [
    { provider: "opencode", model: "a", fields: [], sources: [] },
    { provider: "opencode", model: "b", fields: [], sources: [] },
  ],
});

test("a run on the user's own unrated model says so; a rated one reads as before", () => {
  const now = 3 * HOUR + 1;
  assert.equal(researchLine(ran(true), now), "Last ran 3 hours ago on opencode/kimi (unrated) · 2 models re-rated");
  assert.equal(researchLine(ran(true, "researching"), now), "Researching now on opencode/kimi (unrated)…");
  assert.equal(researchLine(ran(false), now), "Last ran 3 hours ago on opencode/kimi · 2 models re-rated");
});
