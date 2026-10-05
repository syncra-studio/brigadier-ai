import assert from "node:assert/strict";
import { test } from "node:test";

import { shownCommand } from "@/components/transcript/activity";

test("a command drops its shell wrapper, the gate folder and git's display settings", () => {
  assert.equal(
    shownCommand("/bin/zsh -lc '/Users/me/Library/Application Support/Brigadier/gate/bin/git -c core.splitIndex=false -c color.ui=never push origin main'"),
    "git push origin main",
  );
});

test("git settings that change what a command does stay in view", () => {
  assert.equal(
    shownCommand("git -c core.hooksPath=/tmp/untrusted commit -m ok"),
    "git -c core.hooksPath=/tmp/untrusted commit -m ok",
  );
  assert.equal(
    shownCommand("git -c color.ui=never -c alias.x=!sh -c user.email=a@b.c x"),
    "git -c alias.x=!sh -c user.email=a@b.c x",
  );
});

test("web searches retain their meaning in mixed summaries", async () => {
  const { activityOf, summarize } = await import("@/components/transcript/activity");
  const search = activityOf({ kind: "tool", key: "web", name: "functions.WebSearch", input: '{"query":"React streaming"}', status: "completed", output: null });
  assert.equal(search.done, "Searched the web for React streaming");
  assert.equal(summarize([{ kind: "run", doing: "Running git status", done: "Ran git status" }, search]), "Ran a command, searched the web");
});

test("running actions remain visible outside settled runs and preserve start time", async () => {
  const { threadEntries } = await import("@/components/transcript/activity");
  const { TranscriptFolder } = await import("@/components/transcript/transcript");
  const folder = new TranscriptFolder();
  const raw = [
    { streamSeq: 1, atMs: 1, event: { type: "command" as const, itemId: "one", command: "git status", cwd: null, status: "completed" as const, exitCode: 0, output: "", durationMs: 2 } },
    { streamSeq: 2, atMs: 2, event: { type: "command" as const, itemId: "two", command: "pnpm test", cwd: null, status: "inProgress" as const, exitCode: null, output: "", durationMs: null } },
  ];
  const folded = folder.push(raw).items;
  const entries = threadEntries(folded);
  assert.equal(entries.length, 2);
  assert.ok(entries[1]?.kind === "actions");
  assert.equal(entries[1].items[0]?.status, "inProgress");
  const first = folded.find((item) => item.kind === "command" && item.key === "command:two");
  assert.ok(first?.kind === "command");
  assert.equal(first.startedAtMs, 2);
});


test("a command carried by a namespaced tool still names the command", async () => {
  const { activityOf } = await import("@/components/transcript/activity");
  const activity = activityOf({ kind: "tool", key: "cmd", name: "functions.exec_command", input: '{"cmd":"/bin/zsh -lc \'git status\'"}', status: "completed", output: "clean" });
  assert.equal(activity.done, "Ran git status");
  assert.equal(activity.kind, "run");
});
