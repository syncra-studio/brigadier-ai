import assert from "node:assert/strict";
import { test } from "node:test";

import { nextShown, type ProviderTransition } from "@/app/conversation/providerTransitions";
import type { ModelChoice } from "@/ipc/generated";

const claude: ModelChoice = { provider: "claude", model: "opus", effort: "high" };
const codex: ModelChoice = { provider: "codex", model: "gpt", effort: "high" };
const toCodex = (startedAtMs: number) => ({ to: codex, startedAtMs }) as ProviderTransition;

test("a move to another CLI shows in the composer only with its confirmed switch row", () => {
  let shown = nextShown(null, claude, [], true, 1_000);
  assert.equal(shown.shown, claude);
  // The setup names Codex before Codex took the turn: Claude stays shown.
  shown = nextShown(shown, codex, [], true, 2_000);
  assert.equal(shown.shown, claude);
  shown = nextShown(shown, codex, [], true, 9_000);
  assert.equal(shown.shown, claude);
  // An old move to Codex (before this hold) confirms nothing.
  shown = nextShown(shown, codex, [toCodex(-10_000)], true, 9_500);
  assert.equal(shown.shown, claude);
  // Its switch row: Codex shows, in the same render.
  shown = nextShown(shown, codex, [toCodex(-10_000), toCodex(9_800)], true, 10_000);
  assert.equal(shown.shown, codex);
  assert.equal(shown.holdingSinceMs, null);
});

test("a move that never took the turn shows what the next turn starts on once the session stops", () => {
  let shown = nextShown(null, claude, [], true, 1_000);
  shown = nextShown(shown, codex, [], true, 2_000);
  assert.equal(shown.shown, claude);
  shown = nextShown(shown, codex, [], false, 3_000);
  assert.equal(shown.shown, codex);
});

test("a change on the same CLI (another model or effort) shows at once", () => {
  let shown = nextShown(null, claude, [], true, 1_000);
  const sonnet = { ...claude, model: "sonnet" };
  shown = nextShown(shown, sonnet, [], true, 2_000);
  assert.equal(shown.shown, sonnet);
});
