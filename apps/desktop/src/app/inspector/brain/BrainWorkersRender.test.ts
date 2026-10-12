import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

type Row = { name: string; low: boolean; use: string };
type Rendering = {
  first: { picker: string; rows: Row[] };
  updated: Row[];
  reloaded: Row[];
  raced: Row[];
  overtaken: Row[];
  calls: string[];
};

async function render(t: Parameters<typeof renderFixturePage>[0], query: string): Promise<Rendering> {
  // The page's dump is HTML: its ">" comes back escaped.
  const dump = await renderFixturePage(t, `brain-workers.html?drive=1&${query}`, "brain-workers-result", 8000);
  return JSON.parse(dump.replaceAll("&gt;", ">").replaceAll("&lt;", "<").replaceAll("&amp;", "&")) as Rendering;
}

const added = (queries: number, factsOut: number, calls: number) =>
  `asked the Brain ${queries} times · its brief carried 2 facts · taught it ${factsOut} ${factsOut === 1 ? "fact" : "facts"} · ${calls} calls`;
const longRefactor: Row = {
  name: "task-2 Long refactor",
  low: true,
  use: "asked the Brain 0 times · its brief carried 0 facts · taught it 0 facts · 80 calls",
};

// The real Brain tab in Chromium over a faked daemon. Settings opened from a session, with no
// Brain picked before, shows that session's project's Brain and its workers' counters: one read
// of the session's tasks, kept current by task events (an older copy never rolls one back), and
// read again as the bridge resyncs: the read brings back an update the feed missed even at the
// moment of one it delivered, and an update that arrived during the read is applied again; of
// two overlapping reads, the older answering last keeps nothing.
test("the Brain tab opened from a session lists its workers and keeps them current", { timeout: 60000 }, async (t) => {
  const rendering = await render(t, "from=session");
  assert.deepEqual(rendering.first, {
    picker: "calc",
    rows: [{ name: "task-1 Add mul", low: false, use: added(3, 1, 40) }, longRefactor],
  });
  const updated = [{ name: "task-1 Add mul", low: false, use: added(4, 2, 45) }, longRefactor];
  assert.deepEqual(rendering.updated, updated);
  assert.deepEqual(rendering.reloaded, [{ name: "task-1 Add mul", low: false, use: added(5, 2, 50) }, longRefactor]);
  assert.deepEqual(rendering.raced, [{ name: "task-1 Add mul", low: false, use: added(6, 3, 55) }, longRefactor]);
  assert.deepEqual(rendering.overtaken, [{ name: "task-1 Add mul", low: false, use: added(8, 3, 65) }, longRefactor]);
  assert.deepEqual(rendering.calls, [
    "getBrain calc",
    ...Array.from({ length: 5 }, () => "getConversation session"),
  ]);
});

test("the Brain tab lists no workers for a chat, another project's session or another Brain picked", { timeout: 60000 }, async (t) => {
  const chat = await render(t, "from=chat");
  assert.deepEqual(chat.first, { picker: "Personal Brain", rows: [] });
  assert.deepEqual(chat.calls, ["getBrain personal"]);

  const other = await render(t, "from=other&pick=calc");
  assert.deepEqual(other.first, { picker: "calc", rows: [] });
  assert.deepEqual(other.calls, ["getBrain calc"]);

  // A Brain picked before is kept over the session's own.
  const picked = await render(t, "from=session&pick=personal");
  assert.deepEqual(picked.first, { picker: "Personal Brain", rows: [] });
  assert.deepEqual(picked.calls, ["getBrain personal"]);
});
