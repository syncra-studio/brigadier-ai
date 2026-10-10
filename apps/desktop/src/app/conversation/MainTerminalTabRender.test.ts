import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

test("a terminal tab says its shell exited until a reconnect or Restart opens a new one", async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "main-terminal.html", "main-terminal-result", 10000)) as {
    opened: string | null;
    exited: { notice: string | null; restart: boolean };
    reconnected: { notice: string | null; shell: string | null };
    restarted: { notice: string | null; shell: string | null };
    opens: { fresh: boolean; id: string }[];
  };
  assert.equal(seen.opened, null);
  assert.deepEqual(seen.exited, { notice: "The shell exited.", restart: true });
  // A reconnect reattaches; the daemon starts a new shell, which hasn't exited.
  assert.deepEqual(seen.reconnected, { notice: null, shell: "shell-2" });
  // Restart starts a fresh shell in the tab.
  assert.deepEqual(seen.restarted, { notice: null, shell: "shell-3" });
  assert.deepEqual(seen.opens, [{ fresh: true, id: "shell-1" }, { fresh: false, id: "shell-2" }, { fresh: true, id: "shell-3" }]);
});
