import assert from "node:assert/strict";
import { test } from "node:test";

import { Changes } from "./changes";
import type { PluginItemView, PluginsView } from "./types";

const item = (key: string, on: boolean): PluginItemView => ({
  key,
  kind: "server",
  name: key.split(":")[1]!,
  description: "",
  detail: "run",
  url: null,
  icon: null,
  on,
  scope: { type: "allProjects" },
  problem: null,
  locked: null,
  removal: "none",
  canShare: false,
  skillCount: 0,
  files: [],
});

const view = (...items: PluginItemView[]): PluginsView => ({ projects: [], items, elsewhere: [] });

/** A change the test settles by hand. */
function later() {
  let settle!: (error?: Error) => void;
  const done = new Promise<void>((resolve, reject) => {
    settle = (error) => (error ? reject(error) : resolve());
  });
  return { work: () => done, settle };
}

test("a failed change's error stays with its item: gone on leaving, or once the next change works", async () => {
  let reads = 0;
  const changes = new Changes(async () => void reads++);
  // pencil's projects change fails on its page.
  await changes.run("server:pencil", "item:server:pencil", {}, async () => {
    throw new Error("Nothing was changed: Claude Code: it is off in every project after the change.");
  });
  assert.equal(reads, 1, "the page is read again after a change, failed or not");
  assert.match(changes.failure("item:server:pencil")!, /Nothing was changed/);
  // Not on railway's page, nor on the list.
  assert.equal(changes.failure("item:server:railway"), null);
  assert.equal(changes.failure("list"), null);
  // Leaving pencil for railway: it's over, also when coming back.
  changes.leave("item:server:railway");
  assert.equal(changes.failure("item:server:pencil"), null);

  // On the list: a failed switch, then one that works, takes the error away.
  changes.leave("list");
  await changes.run("server:playwright", "list", { on: false }, async () => {
    throw new Error("Codex said no");
  });
  assert.equal(changes.failure("list"), "Codex said no");
  changes.leave("list");
  assert.equal(changes.failure("list"), "Codex said no", "staying where it was asked keeps it");
  await changes.run("server:railway", "list", { on: false }, async () => {});
  assert.equal(changes.failure("list"), null);
});

test("queued switches, scope and generic changes run in order while other items run independently", async () => {
  const changes = new Changes(async () => {});
  const shown = (key: string, from: PluginsView) => changes.apply(from).items.find((entry) => entry.key === key)!;
  const page = view(item("server:pencil", true), item("server:railway", true));
  const saved = item("server:pencil", true);
  const scope = { type: "someProjects" as const, projectIds: ["p1"], folders: [] };
  const ran: string[] = [];

  const first = later();
  const running = changes.run("server:pencil", "list", { on: false }, async () => {
    ran.push("off");
    await first.work();
    saved.on = false;
  });
  assert.equal(shown("server:pencil", page).on, false, "the switch moves before the agents take it");
  assert.equal(changes.pending("server:pencil"), true);

  // Another item goes at once.
  let railway = false;
  await changes.run("server:railway", "list", { on: false }, async () => void (railway = true));
  assert.equal(railway, true);

  // Off is running, then On and Some are requested: neither may replace the other.
  const second = later();
  const on = changes.run("server:pencil", "list", { on: true }, async () => {
    ran.push("on");
    await second.work();
    saved.on = true;
  });
  const some = changes.run("server:pencil", "list", { scope }, async () => {
    ran.push("scope");
    saved.scope = scope;
  });
  const generic = changes.run("server:pencil", "list", {}, async () => void ran.push("generic"));
  assert.deepEqual(ran, ["off"], "nothing runs beside this item's running change");
  assert.equal(shown("server:pencil", page).on, true);
  assert.deepEqual(shown("server:pencil", page).scope, scope);
  first.settle();
  await running;
  assert.deepEqual(ran, ["off", "on"]);
  assert.equal(changes.pending("server:pencil"), true);
  assert.equal(shown("server:pencil", page).on, true, "queued intent stays visible between actions");
  assert.deepEqual(shown("server:pencil", page).scope, scope);
  second.settle();
  await Promise.all([on, some, generic]);
  assert.deepEqual(ran, ["off", "on", "scope", "generic"]);
  assert.equal(saved.on, true, "the agents' final state matches the optimistic switch");
  assert.deepEqual(saved.scope, scope, "the agents' final scope matches the optimistic scope");
  // Done: the page as read shows again.
  assert.equal(changes.pending("server:pencil"), false);
  assert.equal(shown("server:pencil", page).on, true);
});

test("a failure arriving after leaving and returning cannot revive an error", async () => {
  const changes = new Changes(async () => {});
  const pencil = "item:server:pencil";
  const railway = "item:server:railway";
  changes.leave(pencil);
  const first = later();
  const running = changes.run("server:pencil", pencil, { on: false }, first.work);
  changes.leave(railway);
  changes.leave(pencil);
  first.settle(new Error("late pencil failure"));
  await running;
  assert.equal(changes.failure(pencil), null);
  assert.equal(changes.failure(railway), null);
  assert.equal(changes.pending("server:pencil"), false);

  // New requests on the returned page can still report and clear their own error.
  await changes.run("server:pencil", pencil, {}, async () => {
    throw new Error("current pencil failure");
  });
  assert.equal(changes.failure(pencil), "current pencil failure");
  await changes.run("server:pencil", pencil, {}, async () => {});
  assert.equal(changes.failure(pencil), null);
});

test("stale results cannot replace or clear another item's current error", async () => {
  for (const error of [undefined, new Error("stale pencil failure")]) {
    const changes = new Changes(async () => {});
    const pencil = "item:server:pencil";
    const railway = "item:server:railway";
    changes.leave(pencil);
    const first = later();
    const running = changes.run("server:pencil", pencil, {}, first.work);
    changes.leave(railway);
    await changes.run("server:railway", railway, {}, async () => {
      throw new Error("current railway failure");
    });
    assert.equal(changes.pending("server:pencil"), true, "railway finishes while pencil is pending");
    first.settle(error);
    await running;
    assert.equal(changes.failure(railway), "current railway failure");
    assert.equal(changes.failure(pencil), null);
  }
});

test("queued requests also lose their error after leaving and returning", async () => {
  const changes = new Changes(async () => {});
  const place = "item:server:pencil";
  changes.leave(place);
  const first = later();
  const running = changes.run("server:pencil", place, { on: false }, first.work);
  const queued = changes.run("server:pencil", place, { on: true }, async () => {
    throw new Error("queued before leaving");
  });
  changes.leave("list");
  changes.leave(place);
  first.settle();
  await Promise.all([running, queued]);
  assert.equal(changes.failure(place), null);
  assert.equal(changes.pending("server:pencil"), false);
});
