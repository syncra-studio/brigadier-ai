import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixtureLive } from "@/fixtures/headless";

type Frame = { top: number; under: string };
type Open = { before: number; frames: Frame[] };

/** The frames as runs: "846px ×37 (Reading the repository…)". */
function runs(frames: readonly Frame[]): string[] {
  const out: { frame: Frame; count: number }[] = [];
  for (const frame of frames) {
    const last = out.at(-1);
    if (last && last.frame.top === frame.top && last.frame.under === frame.under) last.count += 1;
    else out.push({ frame, count: 1 });
  }
  return out.map(({ frame, count }) => `${frame.top}px ×${count}${frame.under ? ` (${frame.under})` : ""}`);
}

// A project's New session holds its composer still while its repository is read (the card is
// bottom-anchored, so a line under it would lift it): opened from a conversation on the first
// open of the run and on one after, and from the new chat. A real problem is still told.
test("a project's new session keeps its composer in place while the repository is read; a checkout on no branch is still told", { timeout: 120000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "composer-new-session.html", "composer-new-session-result", 90000)) as
    { error?: string } & Record<"cold" | "repeat" | "fromChat" | "detached", Open>;
  assert.equal(seen.error, undefined, String(seen.error));
  const opens = ["cold", "repeat", "fromChat"] as const;
  for (const name of [...opens, "detached"] as const) {
    assert.ok(seen[name].frames.length > 10, `${name}: too few frames recorded (${seen[name].frames.length})`);
  }
  const shown = Object.fromEntries(opens.map((name) => [name, runs(seen[name].frames)]));
  const still = Object.fromEntries(opens.map((name) => [name, [`${seen[name].before}px ×${seen[name].frames.length}`]]));
  assert.deepEqual(shown, still, "the composer card's top left where it was before the click");
  // Nothing under the card while it reads; then the problem, once it is known.
  const detached = runs(seen.detached.frames);
  assert.equal(detached.length, 2, detached.join(", "));
  assert.equal(detached[0], `${seen.detached.before}px ×${detached[0]!.split("×")[1]}`);
  assert.match(detached[1]!, /\(Pick the base branch\.\)$/);
});
