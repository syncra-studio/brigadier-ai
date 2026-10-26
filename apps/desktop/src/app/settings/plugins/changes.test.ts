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
  await changes.run("server:playwright", "list", { on: false }, async () => {
    throw new Error("Codex said no");
  });
  assert.equal(changes.failure("list"), "Codex said no");
  changes.leave("list");
  assert.equal(changes.failure("list"), "Codex said no", "staying where it was asked keeps it");
  await changes.run("server:railway", "list", { on: false }, async () => {});
  assert.equal(changes.failure("list"), null);
});

test("a change shows at once, items don't wait on each other, and a newer change replaces one waiting", async () => {
  const changes = new Changes(async () => {});
  const shown = (key: string, from: PluginsView) => changes.apply(from).items.find((entry) => entry.key === key)!;
  const page = view(item("server:pencil", true), item("server:railway", true));

  const first = later();
  const running = changes.run("server:pencil", "list", { on: false }, first.work);
  assert.equal(shown("server:pencil", page).on, false, "the switch moves before the agents take it");
  assert.equal(changes.pending("server:pencil"), true);

  // Another item goes at once.
  let railway = false;
  await changes.run("server:railway", "list", { on: false }, async () => void (railway = true));
  assert.equal(railway, true);

  // Two more for pencil while the first runs: only the newest of them runs, after it.
  const ran: string[] = [];
  const replaced = changes.run("server:pencil", "list", { on: true }, async () => void ran.push("on"));
  const newest = changes.run("server:pencil", "list", { scope: { type: "someProjects", projectIds: ["p1"], folders: [] } }, async () => void ran.push("scope"));
  await replaced;
  assert.deepEqual(ran, [], "nothing runs beside the running change");
  assert.deepEqual(shown("server:pencil", page).scope, { type: "someProjects", projectIds: ["p1"], folders: [] });
  first.settle();
  await running;
  await newest;
  assert.deepEqual(ran, ["scope"]);
  // Done: the page as read shows again.
  assert.equal(changes.pending("server:pencil"), false);
  assert.equal(shown("server:pencil", page).on, true);
});
