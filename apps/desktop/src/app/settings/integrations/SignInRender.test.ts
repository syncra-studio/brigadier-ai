import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

type Row = { title: string; line: string; end: string };
type SignIn = { title: string; line: string; tile: string; spinner: boolean; actions: string[]; rows: Row[] };
type State = { title: string; description: string; sections: string[]; text: string; tiles: string[]; keyTiles: string[]; overflow: boolean; signIn: SignIn | null };

const NOTE = "A sign-in made in Brigadier is used from the next time a session starts or resumes, not in one already mid-turn.";
const SECTION = "Each agent keeps its own sign-in. Brigadier reuses one from whichever agent has it, and never shows a token.";
const row = (title: string, line: string, end = ""): Row => ({ title, line, end });
const claudeSignedIn = row("Claude Code", "Signed in through Claude Code's own setup", "Signed in");
const codexLogin = row("Codex", "Not signed in. Or sign in for Codex yourself, in a terminal:", "codex mcp login notion");
const needed = { title: "Needs a sign-in", line: "No agent has a sign-in Brigadier can reuse, so no worker can use it yet. Sign in once here and it serves every agent's workers.", tile: "bg-warning/15", spinner: false, actions: ["Sign in in Brigadier"], rows: [row("Claude Code", "Not signed in"), codexLogin] };
const waiting = (rows: Row[]) => ({ title: "Waiting for you in the browser", tile: "bg-foreground/5", spinner: true, actions: ["Open the page again", "Cancel"], rows });
const willUse = (title: string) => row(title, "Will use the sign-in you're making in Brigadier", "Waiting");
const brigadier = { title: "Every worker can use it", line: "Signed in once in Brigadier on 21 Oct. That sign-in serves Claude Code and Codex workers.", tile: "bg-success/15", spinner: false, actions: ["Sign out"], rows: [row("Claude Code", "Uses the sign-in you made in Brigadier", "Uses Brigadier's"), row("Codex", "Uses the sign-in you made in Brigadier", "Uses Brigadier's")] };
const refused = { title: "Only Claude workers can use it", line: "macOS asked whether Brigadier may read Claude Code's saved sign-in, and it wasn't allowed. Codex can't reuse it until you allow it — or sign in once here instead.", tile: "bg-warning/15", spinner: false, actions: ["Ask macOS again", "Sign in in Brigadier"], rows: [claudeSignedIn, row("Codex", "Can't reuse Claude Code's sign-in: macOS wasn't allowed to let Brigadier read it", "Not allowed")] };

test("plugins sign-in: every state on the Signing in board, the tile and the sign-in actions", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "integrations.html?drive=sign-in", "integrations-result", 20000)) as Record<string, unknown> & Record<string, State>;
  assert.equal(seen.error, undefined);
  const signIn = (state: string) => seen[state]!.signIn!;
  for (const state of ["linear-reusing", "notion-needed", "notion-both", "notion-brigadier", "notion-undated", "notion-restored", "linear-refused", "linear-unknown", "linear-unchecked", "linear-native", "linear-partial", "linear-not-mac"]) {
    const page = seen[state]!;
    assert.deepEqual(page.sections, ["Sign-in", "What it is", "Found in"], state);
    assert.ok(page.text.includes(SECTION) && page.text.includes("macOS only for now") && page.text.endsWith(NOTE), state);
    assert.equal(page.overflow, false, state);
  }
  // ① Screens.
  assert.equal(seen["linear-reusing"]!.description, "Remote address with a sign-in · found in Claude Code");
  assert.deepEqual(signIn("linear-reusing"), { title: "Every worker can use it", line: "Signed in through Claude Code. Brigadier reuses that sign-in for Codex workers, so every worker can use it.", tile: "bg-success/15", spinner: false, actions: [], rows: [claudeSignedIn, row("Codex", "Uses the sign-in from Claude Code. No extra sign-in needed.", "Uses Claude Code's")] });
  assert.equal(seen["notion-needed"]!.description, "Remote address with a sign-in · found in Codex");
  // Claude Code has no notion of its own, so its row offers no /mcp: only Brigadier's sign-in.
  assert.deepEqual(signIn("notion-needed"), needed);
  assert.equal(seen["notion-both"]!.description, "Remote address with a sign-in · found in Claude Code and Codex");
  assert.deepEqual(signIn("notion-both").rows, [row("Claude Code", "Not signed in. Or type this inside Claude Code, then pick notion:", "/mcp"), codexLogin]);
  assert.deepEqual(seen.signing!.signIn, { ...waiting([willUse("Claude Code"), willUse("Codex")]), line: "Finish signing in to notion on the page that opened in your browser. Brigadier waits up to 5 minutes." });
  assert.deepEqual(seen.failed!.signIn, { title: "Sign-in didn't work", line: "The server didn't accept the sign-in. Check you signed in to the right workspace, then try again.", tile: "bg-destructive/10", spinner: false, actions: ["Try again"], rows: needed.rows });
  assert.deepEqual(seen.timedOut!.signIn, { ...seen.failed!.signIn!, line: "Timed out after 5 minutes. Nothing was saved." });
  assert.deepEqual(signIn("notion-brigadier"), brigadier);
  assert.deepEqual(signIn("linear-refused"), refused);
  assert.equal(seen["linear-not-mac"]!.description, "Remote address with a sign-in · found in Claude Code");
  const unknownHere = row("Claude Code", "Brigadier can't read saved sign-ins on this system", "Unknown");
  assert.deepEqual(signIn("linear-not-mac"), { title: "Can't check sign-ins on this system", line: "Brigadier reads saved sign-ins and signs in only on macOS for now. Each agent's own sign-in still works for its own workers.", tile: "bg-foreground/5", spinner: false, actions: [], rows: [unknownHere, { ...unknownHere, title: "Codex" }] });
  // The list: only notion needs a sign-in; linear reuses Claude Code's. Its tile opens the page at Sign-in.
  const list = seen["sign-in-list"]!;
  assert.deepEqual(list.keyTiles, ["server:CodexUser:notion"]);
  assert.equal(list.text.split("Needs a sign-in · no worker can use it yet").length, 2);
  assert.match(list.text, /linearhttps:\/\/mcp\.linear\.app\/mcp/);
  assert.equal(list.overflow, false);
  assert.deepEqual(seen.linearScrolled, []);
  assert.equal(seen.notionOpened!.title, "notion");
  assert.deepEqual(seen.notionScrolled, [true]);
  // States the board doesn't draw.
  assert.deepEqual(signIn("notion-undated"), { ...brigadier, line: "Signed in once in Brigadier. That sign-in serves Claude Code and Codex workers." });
  assert.deepEqual(signIn("notion-restored"), { ...waiting([willUse("Claude Code"), willUse("Codex")]), line: "Finish signing in to notion on the page that opened in your browser. Brigadier waits up to 5 minutes." });
  assert.deepEqual(seen.restoredDone!.signIn, brigadier);
  const codexUnknown = row("Codex", "Not checked yet. Checking may make macOS ask whether Brigadier may read a saved sign-in.", "Unknown");
  assert.deepEqual(signIn("linear-unknown"), { title: "Only Claude workers can use it for now", line: "Brigadier hasn't checked yet whether Codex workers can reuse a sign-in. Check now, or sign in once here.", tile: "bg-warning/15", spinner: false, actions: ["Check now", "Sign in in Brigadier"], rows: [claudeSignedIn, codexUnknown] });
  assert.deepEqual(signIn("linear-unchecked"), { title: "Not checked yet", line: "Brigadier hasn't checked yet whether workers can reuse a sign-in. Check now, or sign in once here.", tile: "bg-warning/15", spinner: false, actions: ["Check now", "Sign in in Brigadier"], rows: [row("Claude Code", "Not signed in. Or type this inside Claude Code, then pick linear:", "/mcp"), codexUnknown] });
  assert.deepEqual(signIn("linear-native"), { title: "Every worker can use it", line: "Signed in through Claude Code and Codex, so every worker can use it.", tile: "bg-success/15", spinner: false, actions: [], rows: [claudeSignedIn, row("Codex", "Signed in through Codex's own setup", "Signed in")] });
  assert.deepEqual(signIn("linear-partial"), { title: "Only Claude workers can use it", line: "Codex has no sign-in Brigadier can reuse, so Codex workers can't use it yet. Sign in once here and it serves every agent's workers.", tile: "bg-warning/15", spinner: false, actions: ["Sign in in Brigadier"], rows: [claudeSignedIn, row("Codex", "Not signed in")] });
  // Cancelling goes back to what the agents' states say, keeping Claude Code's own sign-in.
  assert.deepEqual(seen.cancelledNeeded!.signIn, needed);
  assert.deepEqual(seen.signingRefused!.signIn, { ...waiting([claudeSignedIn, willUse("Codex")]), line: "Finish signing in to linear on the page that opened in your browser. Brigadier waits up to 5 minutes." });
  assert.deepEqual(seen.cancelledRefused!.signIn, refused);
  assert.deepEqual(seen.calls, [
    "signInMcpServer CodexUser:notion",
    "reopenMcpSignIn sign-in-1",
    "signInMcpServer CodexUser:notion",
    "signInMcpServer CodexUser:notion",
    "cancelMcpSignIn sign-in-3",
    "askMcpSignInAgain ClaudeUser:linear",
    "signInMcpServer ClaudeUser:linear",
    "cancelMcpSignIn sign-in-4",
    "askMcpSignInAgain ClaudeUser:linear",
    "signOutMcpServer CodexUser:notion",
  ]);
});
