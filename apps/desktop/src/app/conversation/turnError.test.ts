import assert from "node:assert/strict";
import { test } from "node:test";

import { cutOff, describeTurnError, interruptedByQuit, newestFailedError, withoutTurnError } from "@/app/conversation/turnError";
import type { Notice, UserRequest } from "@/ipc/generated";

const summary = (raw: string, provider?: string) => describeTurnError(raw, provider).summary;

test("a failed turn reads as one plain line, chosen from the raw error", () => {
  assert.equal(summary("Claude's 5-hour usage limit was reached. It resets at 4pm."), "Claude's usage limit was reached.");
  assert.equal(summary("You've hit your limit · resets 4pm", "Codex"), "Codex's usage limit was reached.");
  assert.equal(summary("API Error: Connection error."), "Lost the connection to Claude.");
  assert.equal(summary("API Error: Request timed out."), "Lost the connection to Claude.");
  assert.equal(summary("fetch failed: ECONNRESET"), "Lost the connection to Claude.");
  // The daemon's own words when the CLI's process went away mid-turn.
  assert.equal(summary("The CLI session ended unexpectedly."), "Claude stopped unexpectedly.");
  assert.equal(summary("codex exited: Killed: 9", "Codex"), "Codex stopped unexpectedly.");
  assert.equal(
    summary('API Error: 500 {"type":"error","error":{"type":"api_error","message":"Internal server error"}}'),
    "This turn stopped because of an error on Claude's side.",
  );
  assert.equal(summary('API Error: 529 {"type":"error","error":{"type":"overloaded_error"}}'), "Claude is overloaded right now.");
  // By kind, as a worker's error is: a rate limit (no usage limit), a context too long, a sign-in.
  assert.equal(summary("API Error: 429 rate_limit_error: Rate limit exceeded"), "Claude is limiting how fast it takes requests. Try again in a moment.");
  assert.equal(describeTurnError("Rate limit exceeded").limit, undefined);
  assert.equal(summary("API Error: 400 prompt is too long: 210000 tokens > 200000 maximum"), "The conversation grew too long for Claude's context.");
  assert.equal(summary("context_length_exceeded", "Codex"), "The conversation grew too long for Codex's context.");
  assert.equal(summary('API Error: 401 {"type":"error","error":{"type":"authentication_error"}}'), "Claude needs you to sign in again.");
  assert.equal(summary("Not logged in · Please run /login"), "Claude needs you to sign in again.");
  assert.equal(summary("The reply failed."), "This turn stopped because of an error.");
  assert.equal(summary(""), "This turn stopped because of an error.");
});

test("the line never repeats the raw error", () => {
  for (const raw of ["API Error: 500 Internal server error", "The CLI session ended unexpectedly.", "boom"]) {
    assert.ok(!summary(raw).includes(raw));
  }
});

test("the newest failed turn's error is the one the thread shows", () => {
  assert.equal(newestFailedError([]), null);
  assert.equal(newestFailedError([{ end: "done", error: null }]), null);
  assert.equal(
    newestFailedError([
      { end: "failed", error: "old" },
      { end: "interrupted", error: "new" },
      { end: "done", error: null },
    ]),
    "new",
  );
});

const notice = (text: string, atMs: number): Notice => ({ level: "warning", text, atMs });

test("a notice repeating the failed turn's error is dropped, others stay", () => {
  const error = "API Error: 500 Internal server error";
  const notices = [notice("Sandbox unavailable: running without it.", 1), notice(`${error}\n`, 2)];
  assert.deepEqual(withoutTurnError(notices, error), [notices[0]]);
  assert.equal(withoutTurnError(notices, null), notices);
  assert.equal(withoutTurnError(notices, "something else"), notices);
});

test("a usage limit is said as such, for the thread's divider", () => {
  assert.equal(describeTurnError("You've hit your usage limit", "Claude").limit, true);
  assert.equal(describeTurnError("You've hit your session limit · resets 3:10pm", "Claude").limit, true);
  assert.equal(describeTurnError("ECONNRESET", "Claude").limit, undefined);
});

test("a turn Brigadier quit during reads interrupted, also one still starting, which Continue sends again", () => {
  assert.equal(summary("Brigadier quit while this was running."), "Interrupted: Brigadier quit while this was running. Continue picks it up where it stands.");
  assert.equal(summary("Brigadier quit before this started."), "Interrupted: Brigadier quit before this started. Continue sends it again.");
  assert.equal(interruptedByQuit("Brigadier quit before this started."), true);
  assert.equal(interruptedByQuit("The CLI session ended unexpectedly."), false);
});

const request = (id: string, state: UserRequest["state"]) => ({ id, state });

test("a turn the thread says was cut reads by its requests: a quit, with Continue, only when its request was cut by one", () => {
  const quit = "Brigadier quit while this was running.";
  const cut = request("r1", { type: "failed", error: quit });
  // Brigadier quit under the newest request: Continue picks it up.
  assert.deepEqual(cutOff(quit, [cut], cut), { error: quit, resumable: true });
  const started = request("r1", { type: "failed", error: "Brigadier quit before this started." });
  assert.deepEqual(cutOff(quit, [started], started), { error: "Brigadier quit before this started.", resumable: true });
  // A request after it: still a quit, but the daemon resumes only the newest.
  assert.deepEqual(cutOff(quit, [cut], request("r2", { type: "done" })), { error: quit, resumable: false });
  // The CLI went away under it, Brigadier ran on: its request's own words, no Continue.
  const ended = request("r1", { type: "failed", error: "The CLI session ended unexpectedly." });
  assert.deepEqual(cutOff(quit, [ended], ended), { error: "The CLI session ended unexpectedly.", resumable: false });
  assert.equal(summary(cutOff(quit, [ended], ended).error), "Claude stopped unexpectedly.");
  // A turn of Brigadier's own (its `/compact`, the freelogo thread at 11:08), or one whose request
  // was answered: no quit, and nothing for Continue to pick up.
  const done = request("r1", { type: "done" });
  assert.deepEqual(cutOff(quit, [], done), { error: "The CLI session ended unexpectedly.", resumable: false });
  assert.deepEqual(cutOff(quit, [done], done), { error: "The CLI session ended unexpectedly.", resumable: false });
  // Any other error is the turn's own.
  assert.deepEqual(cutOff("The API refused it.", [cut], cut), { error: "The API refused it.", resumable: false });
});
