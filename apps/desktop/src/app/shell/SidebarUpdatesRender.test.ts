import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive, renderFixturePage } from "@/fixtures/headless";

// The real sidebar in Chromium over a faked daemon: the Chats section's "+" next to Projects',
// a lone agent update run from the chip itself, two updates where the finished one leaves the
// open list by itself, and an update that can't start opening the list on why.
test("sidebar: Chats +, updates that run from the chip and leave by themselves", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "updates.html?drive=1", "updates-result", 20000)) as Record<string, unknown>;
  assert.deepEqual(seen.chatsPlus, { there: true, sameLook: true });
  assert.deepEqual(seen.selection, { type: "draft", kind: "chat" });

  assert.equal(seen.loneTip, "Update Codex to 1.1.0");
  assert.equal(seen.loneMenu, false, "a lone agent update runs without the list");
  assert.deepEqual(seen.loneCalls, [{ method: "runUpdate", target: "codex" }]);
  assert.equal(seen.loneUpdating, "Updating Codex");
  assert.equal(seen.loneUpdated, "Codex updated");
  assert.equal(seen.loneAfter, "gone");

  assert.deepEqual(seen.twoRows, ["Claude Code", "Codex"]);
  assert.equal(seen.twoCalls, 2);
  assert.deepEqual(seen.finishedShowing, ["Claude Code", "Codex"], "a finished row shows its check a moment");
  assert.deepEqual(seen.afterFirst, { rows: ["Claude Code"], open: true });
  assert.deepEqual(seen.afterLast, { open: false, pill: "gone" });

  assert.deepEqual(seen.failed, { open: true, alert: "Couldn't reach npm" });
});

// The narrowest sidebar with the agents' updates pill and a nightly update ready: the restart
// pill stays inside, clear of the other pill, rather than past the sidebar's edge or over it.
test("sidebar: the restart pill fits a narrow sidebar", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "updates.html?narrow=1", "updates-result")) as Record<string, unknown>;
  assert.deepEqual(seen, { open: "expanded", inside: true, apart: true });
});

type Box = { label: string; x: number; y: number; width: number; height: number; opacity: number; icons: number };
type Frame = { ms: number; boxes: Box[] };

const PILL = /Brigadier|Restart to update/;
const at = (frame: Frame, label: string | RegExp) =>
  frame.boxes.find((box) => (typeof label === "string" ? box.label === label : label.test(box.label)));
/** A box's place and size, without its name or fade. */
const footprint = (box: Box | undefined) => box && { x: box.x, y: box.y, width: box.width, height: box.height };

// Recorded frame by frame in real time (#387): a nightly update the shell finds shows as a pill
// beside the agents' one and nothing downloads; it grows in as one smooth motion, its neighbours
// gliding aside rather than jumping; a click turns it busy and then into "Restart to update"
// without it or any other icon moving or resizing, its icon there on every frame; a click on that
// restarts.
test("sidebar: Brigadier's update pill goes from update to restart without a jump", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "updates.html?frames=1", "app-update-frames")) as {
    appear: Frame[];
    updating: Frame[];
    calls: string[];
  };
  const { appear, updating } = seen;
  assert.ok(appear.length > 20 && updating.length > 20, `frames came (${appear.length}, ${updating.length})`);

  // Appearing: before it the shell has downloaded nothing; each icon moves one way, a step at a
  // time, along its row, never across it.
  const last = appear.at(-1)!;
  assert.equal(at(last, PILL)?.label, "Update Brigadier to 0.1.0-nightly.202610200130");
  for (const { label } of last.boxes) {
    const path = appear.map((frame) => at(frame, label)).filter((box): box is Box => box !== undefined);
    const ys = new Set(path.map((box) => box.y));
    assert.equal(ys.size, 1, `${label} stays on its line: ${JSON.stringify([...ys])}`);
    for (const side of ["x", "width"] as const) {
      const values = path.map((box) => box[side]).filter((value, i, all) => i === 0 || value !== all[i - 1]);
      const moved = Math.abs(values.at(-1)! - values[0]!);
      const steps = values.slice(1).map((value, i) => value - values[i]!);
      assert.ok(
        steps.every((step) => Math.sign(step) === Math.sign(steps[0] ?? 0)),
        `${label}'s ${side} changes one way: ${JSON.stringify(values)}`,
      );
      if (moved > 4) {
        assert.ok(Math.max(...steps.map(Math.abs)) < moved * 0.6, `${label}'s ${side} glides rather than jumps: ${JSON.stringify(values)}`);
      }
    }
  }
  // The pill fades in, never back out, and stays drawn once it is.
  const fade = appear.map((frame) => at(frame, PILL)?.opacity ?? 0);
  assert.ok(fade.every((opacity, i) => i === 0 || opacity >= fade[i - 1]!), `fades in one way: ${JSON.stringify(fade)}`);
  assert.ok(new Set(fade.filter((opacity) => opacity > 0 && opacity < 1)).size >= 3, `fades over frames: ${JSON.stringify(fade)}`);
  assert.equal(fade.at(-1), 1);

  // Clicked, busy, ready: the same tips in turn and nothing else; every icon where it was.
  const tips = updating.map((frame) => at(frame, PILL)?.label).filter((tip, i, all) => i === 0 || tip !== all[i - 1]);
  assert.deepEqual(tips, ["Update Brigadier to 0.1.0-nightly.202610200130", "Updating Brigadier", "Restart to update"]);
  for (const frame of updating) {
    for (const box of last.boxes) {
      const now = at(frame, box.label === at(last, PILL)?.label ? PILL : box.label);
      assert.deepEqual(footprint(now), footprint(box), `${box.label} at ${frame.ms} ms`);
      assert.equal(now?.opacity, 1, `${box.label} fully drawn at ${frame.ms} ms`);
    }
    assert.equal(at(frame, PILL)?.icons, 2, `the pill's icon and mark at ${frame.ms} ms`);
  }
  assert.deepEqual(seen.calls, ["take_app_update", "restart_to_update"]);
});

// A download that fails: the pill says so and keeps its download icon; a click tries again.
test("sidebar: a failed update says so and a click tries again", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "updates.html?fail=1", "updates-result")) as Record<string, unknown>;
  assert.deepEqual(seen, {
    failed: "Couldn't update Brigadier. Click to try again",
    again: "Updating Brigadier",
    after: "Restart to update",
    calls: ["take_app_update", "take_app_update"],
  });
});
