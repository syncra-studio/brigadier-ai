import assert from "node:assert/strict";
import { test } from "node:test";

import { nextShown, placeSwitches, type ProviderTransition } from "@/app/conversation/providerTransitions";
import type { ModelChoice } from "@/ipc/generated";

const claude: ModelChoice = { provider: "claude", model: "opus", effort: "high" };
const codex: ModelChoice = { provider: "codex", model: "gpt", effort: "high" };
const toCodex = (id: string) => ({ id, to: codex }) as ProviderTransition;

test("a move to another CLI shows in the composer only with a new switch row", () => {
  let shown = nextShown(null, claude, [], true);
  assert.equal(shown.shown, claude);
  assert.equal(nextShown(shown, claude, [], true), shown, "nothing changed: the same state");
  // The setup names Codex before Codex took the turn; an old move to Codex confirms nothing.
  const old = [toCodex("old")];
  shown = nextShown(shown, codex, old, true);
  assert.equal(shown.shown, claude);
  assert.equal(nextShown(shown, codex, old, true), shown);
  // Its switch row: Codex shows, in the same render.
  shown = nextShown(shown, codex, [...old, toCodex("new")], true);
  assert.equal(shown.shown, codex);
  assert.equal(shown.rowsBefore, null);
});

test("a move that never took the turn shows what the next turn starts on once the session stops", () => {
  let shown = nextShown(null, claude, [], true);
  shown = nextShown(shown, codex, [], true);
  assert.equal(shown.shown, claude);
  shown = nextShown(shown, codex, [], false);
  assert.equal(shown.shown, codex);
});

test("a change on the same CLI (another model or effort) shows at once", () => {
  let shown = nextShown(null, claude, [], true);
  const sonnet = { ...claude, model: "sonnet" };
  shown = nextShown(shown, sonnet, [], true);
  assert.equal(shown.shown, sonnet);
});

const message = (key: string, text: string, role: "user" | "assistant" = "user") =>
  ({ kind: "item", item: { kind: "message", key, role, text, streaming: false } }) as const;

test("a worker's switches show once each, in place of the words it was resumed with", () => {
  const entries = [
    message("brief", "Do the task"),
    message("a1", "Working on it", "assistant"),
    message("r1", "[Quota is available again. Check git status and git log first: …]"),
    message("a2", "Carrying on", "assistant"),
  ];
  const first = toCodex("t1");
  const second = { id: "t2", to: claude } as ProviderTransition;
  const placed = placeSwitches([first, second], entries);
  assert.deepEqual([...placed.at.entries()], [["r1", first]]);
  assert.deepEqual(placed.rest, [second]);
  assert.deepEqual(placeSwitches([], entries).at.size, 0);
});
