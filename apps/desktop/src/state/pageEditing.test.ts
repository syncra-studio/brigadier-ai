import assert from "node:assert/strict";
import { test } from "node:test";

import type { Page, PageContent, PageWriteOutcome } from "@/ipc/generated";

const data = new Map<string, string>();
Object.defineProperty(globalThis, "localStorage", { configurable: true, value: {
  getItem: (key: string) => data.get(key) ?? null,
  setItem: (key: string, value: string) => data.set(key, value),
  removeItem: (key: string) => data.delete(key),
} });

const { createPageEditing, usePageEdits } = await import("./pageEditing");

/** A daemon that keeps pages in memory, with the same revision rule as the real one. */
function fakeDaemon() {
  const pages = new Map<string, Page>();
  let next = 0;
  const writes: { id: string; content: PageContent; base: string }[] = [];
  const put = (id: string, content: PageContent): Page => {
    const page = { id, content, revision: `r${++next}`, createdMs: 0, updatedMs: next };
    pages.set(id, page);
    return page;
  };
  return {
    pages, writes, put,
    ipc: {
      read: async (id: string) => {
        const page = pages.get(id);
        if (!page) throw new Error("No such page");
        return page;
      },
      write: async (id: string, content: PageContent, base: string): Promise<PageWriteOutcome> => {
        writes.push({ id, content, base });
        const page = pages.get(id)!;
        if (page.revision !== base) return { type: "conflict", current: page };
        return { type: "saved", revision: put(id, content).revision };
      },
      create: async () => put(`p${next + 1}`, { title: "", emoji: null, body: "" }),
    },
  };
}

const text = (body: string, title = "") => ({ title, emoji: null, body });
const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

function gate() {
  let resolve!: () => void;
  const promise = new Promise<void>((done) => { resolve = done; });
  return { promise, resolve };
}

test("closing one of two tabs for a page keeps the remaining editor and its saves", async () => {
  data.clear();
  const daemon = fakeDaemon();
  daemon.put("shared", text("base"));
  const editing = createPageEditing(daemon.ipc, 10_000);
  await editing.open("shared");
  await editing.open("shared");
  await editing.close("shared");
  assert.equal(usePageEdits.getState().pages.shared?.content.body, "base");
  editing.edit("shared", text("remaining tab's edit"));
  await editing.flush("shared");
  assert.equal(daemon.pages.get("shared")!.content.body, "remaining tab's edit");
  await editing.close("shared");
  assert.equal(usePageEdits.getState().pages.shared, undefined);
});

test("restoring a deleted page at the same revision resumes saving the recovered edits", async () => {
  data.clear();
  const daemon = fakeDaemon();
  const original = daemon.put("restored-same", text("base"));
  const editing = createPageEditing(daemon.ipc, 10_000);
  await editing.open(original.id);
  editing.edit(original.id, text("mine"));
  daemon.pages.delete(original.id);
  editing.changed(original.id, null);
  assert.equal(usePageEdits.getState().pages[original.id]!.status, "missing");
  daemon.pages.set(original.id, original);
  editing.changed(original.id, original.revision);
  await wait(0);
  assert.equal(usePageEdits.getState().pages[original.id]!.status, "ready");
  assert.equal(usePageEdits.getState().pages[original.id]!.conflict, null);
  await editing.flush(original.id);
  assert.equal(daemon.pages.get(original.id)!.content.body, "mine");
});

test("a restored page at another revision can resolve its conflict and keep saving", async () => {
  data.clear();
  const daemon = fakeDaemon();
  daemon.put("restored-other", text("base"));
  const editing = createPageEditing(daemon.ipc, 10_000);
  await editing.open("restored-other");
  editing.edit("restored-other", text("mine"));
  const changed = daemon.put("restored-other", text("before deletion"));
  editing.changed("restored-other", changed.revision);
  await wait(0);
  daemon.pages.delete("restored-other");
  editing.changed("restored-other", null);
  assert.equal(usePageEdits.getState().pages["restored-other"]!.conflict, null, "a deleted file offers no stale Use theirs choice");
  const restored = daemon.put("restored-other", text("restored elsewhere"));
  editing.changed(restored.id, restored.revision);
  await wait(0);
  assert.equal(usePageEdits.getState().pages[restored.id]!.status, "ready");
  assert.equal(usePageEdits.getState().pages[restored.id]!.conflict?.content.body, "restored elsewhere");
  await editing.keepMine(restored.id);
  assert.equal(daemon.pages.get(restored.id)!.content.body, "mine");
  editing.edit(restored.id, text("next edit"));
  await editing.flush(restored.id);
  assert.equal(daemon.pages.get(restored.id)!.content.body, "next edit");
});

test("reopening while the previous tab flushes keeps the reopened editor", async () => {
  data.clear();
  const daemon = fakeDaemon();
  daemon.put("reopen", text("base"));
  const saving = gate();
  const started = gate();
  const editing = createPageEditing({ ...daemon.ipc, write: async (id, content, base) => {
    started.resolve();
    await saving.promise;
    return daemon.ipc.write(id, content, base);
  } }, 10_000);
  await editing.open("reopen");
  editing.edit("reopen", text("last edit"));
  const closing = editing.close("reopen");
  await started.promise;
  await editing.open("reopen");
  saving.resolve();
  await closing;
  assert.equal(usePageEdits.getState().pages.reopen?.content.body, "last edit");
  editing.edit("reopen", text("next edit"));
  await editing.flush("reopen");
  assert.equal(daemon.pages.get("reopen")!.content.body, "next edit");
});

test("typing while Keep both saves a copy retains the later edits and recovery", async () => {
  data.clear();
  const daemon = fakeDaemon();
  daemon.put("copying", text("base"));
  const saving = gate();
  const started = gate();
  const editing = createPageEditing({ ...daemon.ipc, write: async (id, content, base) => {
    if (id !== "copying") { started.resolve(); await saving.promise; }
    return daemon.ipc.write(id, content, base);
  } }, 10_000);
  await editing.open("copying");
  editing.edit("copying", text("mine"));
  daemon.put("copying", text("theirs"));
  await editing.flush("copying");
  const copying = editing.keepBoth("copying");
  await started.promise;
  editing.edit("copying", text("mine plus later typing"));
  saving.resolve();
  const copy = await copying;
  assert.equal(daemon.pages.get(copy)!.content.body, "mine");
  assert.equal(daemon.pages.get("copying")!.content.body, "theirs");
  assert.equal(usePageEdits.getState().pages.copying!.content.body, "mine plus later typing");
  assert.equal(usePageEdits.getState().pages.copying!.dirty, true);
  assert.ok(data.get("brigadier.page.copying")?.includes("mine plus later typing"));
  await editing.keepMine("copying");
});

test("edits save once typing stops, one save per pause, based on the last revision", async () => {
  data.clear();
  const daemon = fakeDaemon();
  daemon.put("a", text("one"));
  const editing = createPageEditing(daemon.ipc, 20);
  await editing.open("a");
  editing.edit("a", text("one two"));
  editing.edit("a", text("one two three"));
  assert.equal(daemon.writes.length, 0, "nothing is written while typing");
  assert.ok(data.get("brigadier.page.a")?.includes("one two three"), "the recovery copy has every keystroke at once");
  await wait(40);
  await editing.flush("a");
  assert.equal(daemon.writes.length, 1);
  assert.equal(daemon.pages.get("a")!.content.body, "one two three");
  assert.equal(usePageEdits.getState().pages.a!.dirty, false);
  assert.equal(data.has("brigadier.page.a"), false, "a saved page needs no recovery copy");
});

test("a quit inside the save delay still writes the last keystrokes", async () => {
  data.clear();
  const daemon = fakeDaemon();
  daemon.put("q", text(""));
  const editing = createPageEditing(daemon.ipc, 10_000);
  await editing.open("q");
  editing.edit("q", text("last words"));
  await editing.flushAll();
  assert.equal(daemon.pages.get("q")!.content.body, "last words");
});

test("a page changed elsewhere while editing is a conflict; nothing is overwritten", async () => {
  data.clear();
  const daemon = fakeDaemon();
  daemon.put("c", text("base"));
  const editing = createPageEditing(daemon.ipc, 10_000);
  await editing.open("c");
  editing.edit("c", text("mine"));
  daemon.put("c", text("theirs"));
  await editing.flush("c");
  const state = usePageEdits.getState().pages.c!;
  assert.equal(state.conflict?.content.body, "theirs");
  assert.equal(daemon.pages.get("c")!.content.body, "theirs", "their side is untouched");
  assert.equal(state.content.body, "mine", "my side is untouched");
  await editing.keepMine("c");
  assert.equal(daemon.pages.get("c")!.content.body, "mine");
  assert.equal(usePageEdits.getState().pages.c!.conflict, null);
});

test("use theirs and keep both resolve a conflict without losing either side", async () => {
  data.clear();
  const daemon = fakeDaemon();
  daemon.put("k", text("base", "Notes"));
  const editing = createPageEditing(daemon.ipc, 10_000);
  await editing.open("k");
  editing.edit("k", text("mine", "Notes"));
  daemon.put("k", text("theirs", "Notes"));
  editing.changed("k", daemon.pages.get("k")!.revision);
  await wait(0);
  assert.equal(usePageEdits.getState().pages.k!.conflict?.content.body, "theirs");
  const copy = await editing.keepBoth("k");
  assert.equal(daemon.pages.get(copy)!.content.body, "mine");
  assert.equal(daemon.pages.get(copy)!.content.title, "Notes (copy)");
  const state = usePageEdits.getState().pages.k!;
  assert.equal(state.content.body, "theirs");
  assert.equal(state.dirty, false);
  editing.edit("k", text("again", "Notes"));
  daemon.put("k", text("other", "Notes"));
  editing.changed("k", daemon.pages.get("k")!.revision);
  await wait(0);
  editing.takeTheirs("k");
  assert.equal(usePageEdits.getState().pages.k!.content.body, "other");
  assert.equal(data.has("brigadier.page.k"), false);
});

test("an outside change with no unsaved edits reloads the editor", async () => {
  data.clear();
  const daemon = fakeDaemon();
  daemon.put("r", text("before"));
  const editing = createPageEditing(daemon.ipc, 10_000);
  await editing.open("r");
  const version = usePageEdits.getState().pages.r!.version;
  daemon.put("r", text("after"));
  editing.changed("r", daemon.pages.get("r")!.revision);
  await wait(0);
  const state = usePageEdits.getState().pages.r!;
  assert.equal(state.content.body, "after");
  assert.ok(state.version > version, "the editor is told to load it again");
  assert.equal(state.conflict, null);
});

test("our own save's change event is not mistaken for an outside change", async () => {
  data.clear();
  const daemon = fakeDaemon();
  daemon.put("o", text(""));
  const editing = createPageEditing({
    ...daemon.ipc,
    write: async (id, content, base) => {
      const outcome = await daemon.ipc.write(id, content, base);
      // The watcher fires before the write's answer arrives.
      if (outcome.type === "saved") editing.changed(id, outcome.revision);
      return outcome;
    },
  }, 10_000);
  await editing.open("o");
  editing.edit("o", text("typed"));
  await editing.flush("o");
  await wait(0);
  const state = usePageEdits.getState().pages.o!;
  assert.equal(state.conflict, null);
  assert.equal(state.dirty, false);
});

test("a recovery copy newer than the file comes back on the next open and is saved", async () => {
  data.clear();
  const daemon = fakeDaemon();
  const page = daemon.put("x", text("on disk"));
  data.set("brigadier.page.x", JSON.stringify({ base: page.revision, content: text("unsaved before a crash") }));
  usePageEdits.setState({ pages: {} });
  const editing = createPageEditing(daemon.ipc, 10_000);
  await editing.open("x");
  assert.equal(usePageEdits.getState().pages.x!.content.body, "unsaved before a crash");
  await editing.flush("x");
  assert.equal(daemon.pages.get("x")!.content.body, "unsaved before a crash");
});

test("a recovery copy from an older revision shows as a conflict instead of overwriting", async () => {
  data.clear();
  const daemon = fakeDaemon();
  const old = daemon.put("y", text("v1"));
  daemon.put("y", text("v2 from elsewhere"));
  data.set("brigadier.page.y", JSON.stringify({ base: old.revision, content: text("mine on v1") }));
  usePageEdits.setState({ pages: {} });
  const editing = createPageEditing(daemon.ipc, 10_000);
  await editing.open("y");
  const state = usePageEdits.getState().pages.y!;
  assert.equal(state.content.body, "mine on v1");
  assert.equal(state.conflict?.content.body, "v2 from elsewhere");
  await editing.flush("y");
  assert.equal(daemon.pages.get("y")!.content.body, "v2 from elsewhere", "no save while the conflict is open");
});

test("a failed save keeps the text and its recovery copy, and the next edit tries again", async () => {
  data.clear();
  const daemon = fakeDaemon();
  daemon.put("f", text(""));
  let fail = true;
  const editing = createPageEditing({
    ...daemon.ipc,
    write: async (id, content, base) => {
      if (fail) throw new Error("Disk full");
      return daemon.ipc.write(id, content, base);
    },
  }, 10_000);
  await editing.open("f");
  editing.edit("f", text("precious"));
  await editing.flush("f");
  const state = usePageEdits.getState().pages.f!;
  assert.equal(state.error, "Disk full");
  assert.equal(state.dirty, true);
  assert.ok(data.get("brigadier.page.f")?.includes("precious"));
  fail = false;
  editing.edit("f", text("precious!"));
  await editing.flush("f");
  assert.equal(daemon.pages.get("f")!.content.body, "precious!");
  assert.equal(usePageEdits.getState().pages.f!.error, null);
});

test("flush says whether the page on disk holds the editor's text before it is sent", async () => {
  data.clear();
  const daemon = fakeDaemon();
  daemon.put("send", text("base"));
  const editing = createPageEditing(daemon.ipc, 10_000);
  await editing.open("send");
  editing.edit("send", text("typed just now"));
  assert.equal(await editing.flush("send"), null);
  assert.equal(daemon.pages.get("send")?.content.body, "typed just now");

  daemon.put("send", text("changed elsewhere"));
  editing.edit("send", text("typed again"));
  assert.match((await editing.flush("send")) ?? "", /changed elsewhere/);
  assert.equal(daemon.pages.get("send")?.content.body, "changed elsewhere");

  const failing = createPageEditing({ ...daemon.ipc, write: async () => { throw new Error("disk full"); } }, 10_000);
  daemon.put("full", text("base"));
  await failing.open("full");
  failing.edit("full", text("unsaved"));
  assert.match((await failing.flush("full")) ?? "", /disk full/);
});
