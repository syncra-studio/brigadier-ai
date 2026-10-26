import assert from "node:assert/strict";
import { test } from "node:test";

import type { ToolStep } from "@/app/conversation/activity/words";
import { namesId, sendArgs, sessionSent } from "@/app/conversation/sessionSend";
import type { ProviderEvent, Task } from "@/ipc/generated";

test("a `brigadier send` command gives whom it messages and the text, however the shell quotes it", () => {
  assert.deepEqual(sendArgs(`brigadier send 4f2a9c1e "Can you check the schema?"`), { to: "4f2a9c1e", text: "Can you check the schema?" });
  assert.deepEqual(sendArgs(`brigadier send 4f2a9c1e 'It's done' `), null, "an unclosed quote reads nothing");
  assert.deepEqual(sendArgs(`brigadier send 4f2a9c1e 'It'"'"'s done'`), { to: "4f2a9c1e", text: "It's done" });
  assert.deepEqual(sendArgs(`brigadier send 4f2a9c1e Check the docs too`), { to: "4f2a9c1e", text: "Check the docs too" });
  assert.deepEqual(sendArgs(`brigadier send 4f2a9c1e "Say \\"hi\\" and \\$HOME"`), { to: "4f2a9c1e", text: `Say "hi" and $HOME` });
  assert.deepEqual(sendArgs(`brigadier send 4f2a9c1e $'line one\\nline two'`), { to: "4f2a9c1e", text: "line one\nline two" });
  assert.deepEqual(
    sendArgs(`brigadier send 4f2a9c1e "$(cat <<'EOF'\nFirst line\n"quoted" second; line\nEOF\n)"`),
    { to: "4f2a9c1e", text: `First line\n"quoted" second; line` },
  );
  // After a `cd`, inside `bash -lc`, with its full path or an environment word first.
  assert.deepEqual(sendArgs(`cd /tmp && brigadier send 4f2a9c1e "Hi"`), { to: "4f2a9c1e", text: "Hi" });
  assert.deepEqual(sendArgs(`bash -lc 'brigadier send 4f2a9c1e Hi'`), { to: "4f2a9c1e", text: "Hi" });
  assert.deepEqual(sendArgs(`FOO=1 /usr/local/bin/brigadier send 4f2a9c1e "Hi" && echo done`), { to: "4f2a9c1e", text: "Hi" });
  // An answer to a worker's prompt, another command, words that only mention it, two sends.
  assert.equal(sendArgs(`brigadier send 4f2a9c1e --allow`), null);
  assert.equal(sendArgs(`brigadier wait 4f2a9c1e`), null);
  assert.equal(sendArgs(`echo brigadier send 4f2a9c1e hi`), null);
  assert.equal(sendArgs(`brigadier send aaaa1111 one; brigadier send bbbb2222 two`), null);
});

test("an id names a session as `brigadier` reads it: whole, or the start of its last 8", () => {
  const id = "0192aaaa-bbbb-7ccc-8ddd-00004f2a9c1e";
  assert.ok(namesId(id, id));
  assert.ok(namesId(id, "4f2a9c1e"));
  assert.ok(namesId(id, "4f2a"));
  assert.ok(!namesId(id, "4f2"), "too short");
  assert.ok(!namesId(id, "9c1e"), "the end, not the start, of its short id");
});

const call = (command: string, status: ToolStep["status"], output: string | null): { step: ToolStep; event: ProviderEvent } => ({
  step: { type: "tool", itemId: "t1", name: "Bash", detail: command, status, throughPosition: 0, item: "command" },
  event: { type: "toolCall", itemId: "t1", name: "Bash", input: JSON.stringify({ command }), status, output },
});

test("a send to another session is the row of its message: running, sent with its id, or refused with what it printed", () => {
  const command = `brigadier send 4f2a9c1e "Can you check the schema?"`;
  const running = call(command, "inProgress", null);
  assert.deepEqual(sessionSent(running.step, running.event), {
    type: "sessionSent", itemId: "t1", to: "4f2a9c1e", text: "Can you check the schema?", status: "inProgress", messageId: null, refused: null,
  });
  const sent = call(command, "completed", "Sent sm-0123456789ab to session 4f2a9c1e: it works on it.\n");
  assert.equal(sessionSent(sent.step, sent.event)?.messageId, "sm-0123456789ab");
  const refused = call(command, "failed", "refused: 4f2a9c1e is this session\n");
  assert.equal(sessionSent(refused.step, refused.event)?.refused, "refused: 4f2a9c1e is this session");
  // A failure that says nothing of a session, or a call the turn cut off, keeps its own row.
  const failed = call(command, "failed", "brigadier: the daemon isn't running\n");
  assert.equal(sessionSent(failed.step, failed.event), null);
  assert.equal(sessionSent({ ...running.step, status: "failed", stopped: true }, running.event), null);
  // A Codex command carries its command line itself.
  const codex: ProviderEvent = { type: "command", itemId: "t1", command, cwd: null, status: "completed", exitCode: 0, output: "Sent sm-0123456789ab to session 4f2a9c1e: queued into its turn.\n", durationMs: 40 };
  assert.equal(sessionSent(sent.step, codex)?.messageId, "sm-0123456789ab");
});

test("a send to one of the session's workers keeps its own row", () => {
  const worker = call(`brigadier send 0011aa22 "Rebase on main"`, "completed", "Sent to 0011aa22: it works on it.\n");
  assert.equal(sessionSent(worker.step, worker.event), null);
  // Before it printed, the session's workers say whose the id is.
  const running = call(`brigadier send 0011aa22 "Rebase on main"`, "inProgress", null);
  const tasks = { "task-9f-0011aa22": {} as Task };
  assert.equal(sessionSent(running.step, running.event, { tasks, orchestratorSteps: [] }), null);
  assert.notEqual(sessionSent(running.step, running.event, { tasks: {}, orchestratorSteps: [] }), null);
  // A finished call that printed no session's words is no message to a session either.
  const quiet = call(`brigadier send 0011aa22 "Rebase on main"`, "completed", "");
  assert.equal(sessionSent(quiet.step, quiet.event), null);
  // Another command, or one that answers a worker's prompt, is no message.
  const other = call(`brigadier workers`, "completed", "");
  assert.equal(sessionSent(other.step, other.event), null);
  const allow = call(`brigadier send 0011aa22 --allow`, "completed", "Allowed what 0011aa22 asked.\n");
  assert.equal(sessionSent(allow.step, allow.event), null);
});
