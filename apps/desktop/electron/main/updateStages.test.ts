import assert from "node:assert/strict";
import test from "node:test";

import { UpdateStages, unlessStalled, updateChannel, updatesEnabled, type Download } from "./updateStages.ts";

function counting(fail = false) {
  const counts = { downloads: 0, installs: 0, reopened: [] as boolean[] };
  const download: Download = async () => {
    counts.downloads += 1;
    if (fail) throw new Error("no");
    return (reopen) => {
      counts.installs += 1;
      counts.reopened.push(reopen);
    };
  };
  return { counts, download };
}

test("only a configured production build looks for updates, never under --smoke", () => {
  assert.equal(updatesEnabled(false, true, false), true);
  assert.equal(updatesEnabled(true, true, false), false);
  assert.equal(updatesEnabled(false, false, false), false);
  assert.equal(updatesEnabled(false, true, true), false);
});

test("the channel is Nightly or Release only where a feed is configured", () => {
  assert.equal(updateChannel("0.1.0-nightly.202610161908", true), "Nightly");
  assert.equal(updateChannel("0.1.0", true), "Release");
  assert.equal(updateChannel("0.1.0", false), null);
});

test("a found update waits for the user's click", () => {
  const stages = new UpdateStages();
  const { counts, download } = counting();
  assert.deepEqual(stages.offer("1", download), { state: "available", version: "1" });
  assert.equal(stages.offer("1", download), null);
  assert.equal(counts.downloads, 0);
  assert.deepEqual(stages.finish(true), { reopen: false, installing: false });
  assert.equal(counts.installs, 0);
});

test("a clicked update downloads, and installs on quit, once", async () => {
  const stages = new UpdateStages();
  const { counts, download } = counting();
  stages.offer("1", download);
  const seen: string[] = [];
  await stages.take((shown) => seen.push(shown.state));
  assert.deepEqual(seen, ["updating", "ready"]);
  assert.equal(stages.offer("2", download), null, "nothing newer replaces a ready update");
  assert.deepEqual(stages.finish(true), { reopen: false, installing: true });
  assert.deepEqual(stages.finish(true), { reopen: false, installing: false });
  assert.deepEqual(counts.reopened, [false]);
});

test("a failed download is tried again on the next click", async () => {
  const stages = new UpdateStages();
  const failing = counting(true);
  stages.offer("1", failing.download);
  const seen: string[] = [];
  await stages.take((shown) => seen.push(shown.state));
  assert.deepEqual(seen, ["updating", "failed"]);
  await stages.take(() => {});
  assert.equal(failing.counts.downloads, 2);
});

test("a running daemon keeps the update out", async () => {
  const stages = new UpdateStages();
  const { counts, download } = counting();
  stages.offer("1", download);
  await stages.take(() => {});
  assert.deepEqual(stages.finish(false), { reopen: false, installing: false });
  assert.equal(counts.installs, 0);
});

test("only Restart to update asks the install to reopen", async () => {
  const stages = new UpdateStages();
  const { counts, download } = counting();
  stages.offer("1", download);
  await stages.take(() => {});
  stages.restart = true;
  assert.deepEqual(stages.finish(true), { reopen: true, installing: true });
  assert.deepEqual(counts.reopened, [true]);
});

test("a stalled download fails and is cancelled; a slow one finishes", async () => {
  let cancelled = 0;
  const never = new Promise<string>(() => {});
  await assert.rejects(unlessStalled(10, never, () => () => {}, () => { cancelled += 1; }), /no new bytes/);
  assert.equal(cancelled, 1);
  const slow = new Promise<string>((resolve) => setTimeout(() => resolve("done"), 60));
  const value = await unlessStalled(30, slow, (tick) => {
    const every = setInterval(tick, 10);
    return () => clearInterval(every);
  }, () => assert.fail("not cancelled"));
  assert.equal(value, "done");
});
