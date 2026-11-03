import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { createServer, type ViteDevServer } from "vite";

import type * as Words from "@/app/settings/routing/words";
import type { GuidanceRow } from "@/ipc/generated";

// The words need the model names lib/routing reads through the app's modules: loaded by Vite.
let server: ViteDevServer;
let withFirst: typeof Words.withFirst;
before(async () => {
  const globals = globalThis as Record<string, unknown>;
  globals.document ??= { documentElement: {} };
  globals.getComputedStyle ??= () => ({ getPropertyValue: () => "" });
  server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  ({ withFirst } = (await server.ssrLoadModule("/src/app/settings/routing/words.ts")) as typeof Words);
});
after(() => server.close());

const row = (agent: GuidanceRow["agent"], model: string, effort: string | null = null): GuidanceRow => ({ work: "Computer use", agent, model, effort, reason: "" });

// Settings › Computer use's pick: first in the work's order, the rest after it in their order.
test("a picked model goes first in the order, the others kept after it", () => {
  const order = [row("claude", "opus", "high"), row("codex", "default", "high")];
  assert.deepEqual(withFirst(order, row("codex", "gpt-6-luna")), [row("codex", "gpt-6-luna"), ...order]);
  // A model already in the order moves up, keeping its effort.
  assert.deepEqual(withFirst(order, row("codex", "default")), [order[1], order[0]]);
  assert.deepEqual(withFirst([], row("claude", "sonnet")), [row("claude", "sonnet")]);
});
