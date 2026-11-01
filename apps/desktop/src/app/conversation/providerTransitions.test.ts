import assert from "node:assert/strict";
import { test } from "node:test";

import { nextShown, placeSwitches, type ProviderTransition } from "@/app/conversation/providerTransitions";
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

test("a worker's switches show once each, in place of the words it was resumed with", () => {
  const message = (key: string, text: string, role: "user" | "assistant" = "user") =>
    ({ kind: "item", item: { kind: "message", key, role, text, streaming: false } }) as const;
  const entries = [
    message("brief", "Do the task"),
    message("a1", "Working on it", "assistant"),
    message("r1", "[Quota is available again. Check git status and git log first: …]"),
    message("a2", "Carrying on", "assistant"),
  ];
  const first = { id: "t1", to: codex, startedAtMs: 1 } as ProviderTransition;
  const second = { id: "t2", to: claude, startedAtMs: 2 } as ProviderTransition;
  const placed = placeSwitches([first, second], entries);
  assert.deepEqual([...placed.at.entries()], [["r1", first]]);
  assert.deepEqual(placed.rest, [second]);
  assert.deepEqual(placeSwitches([], entries).at.size, 0);
});
