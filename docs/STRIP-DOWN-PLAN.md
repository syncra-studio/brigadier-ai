# Strip-down plan: every Brigadier session is a plain Claude Code or Codex CLI

> **Take, don't rebuild.** We take Claude Code's and Codex's own conversation, session files, subagents,
> background tasks, permission modes and memory files as they are. Brigadier rebuilds none of them. It adds only
> what a CLI can't do by itself: seeing and driving other sessions, previews, the browser, computer use, asking
> the user as a card, and searching the record. This plan only removes and re-wires.

> Status: plan, 2026-10-22, from the user's decisions G1–G21 (project memory `plain-cli-threads-1022`), the
> 10-22 diagnosis (run `20261022-0805-take-over-5-sessions`, `DIAGNOSIS.md`) and the long-sessions research
> (same run, `research/PROPOSAL.md`, `QUESTIONS.md`, `EVIDENCE.md`). The decisions are settled. Where one looks
> hard, §8 says so with evidence; nothing here changes one.
> Revised the same day after the Codex plan review (`msgs/w12-plan-review.md`) and three live user decisions:
> G16 (a `brigadier` CLI, not an MCP server), the Personal Brain deleted outright (S19), and #517 closed by G9.
> Amended the same day with G17 (no stripping of what the plain CLI loads), G18 (model choice is advice:
> `brigadier models`, S13b) and the take-aways of `research/LEAN-CONTEXT.md` and `research/POCOCK.md`.
> Amended again with G19 (a worker belongs to its parent session and is never a sidebar session; this corrects
> G3's "workers are ordinary sessions in the sidebar") and the user's two answers after it: the skill is named
> `/brigade` (G19.Q5) and workers are read-only to the user (G19.Q6). These are memory's Q5 and Q6, not the
> long-sessions Q5 and Q6 cited in §1, S18 and S23.
> Amended again with G20 (the brigade is always on: every session and chat gets the `/brigade` skill and a
> pointer to it, with no invocation and no "New brigade" UI) and G21 (workers land into their parent session,
> never into main by rule; the skill guides, Brigadier enforces nothing).
> Amended again with three take-aways of `research/T3CODE-WORKTREES.md` (§2, §4, §6): the session's environment
> and base branch are recorded (S7, S13), a turn's diff leaves out what a merge brought in (S11), and cleanup
> recognises merged, empty and idle worktrees (S20).
> Amended again with §9, "Excel at what we keep" (the user's 10-22 strategy, one run per kept capability), and
> the user's Q8 on workers' permission prompts (§8, item 14; S13).
> Amended again with G23: research goes to lean workers (`brigadier start --lean`, S13), and the skill guides
> asking once, adversarial reviews and adversarial consultation (S15); §9.1 adds a lean-research bar.
> Amended again on 10-23 with `research/CONTINUITY.md`: decisions survive hand-offs (S22b, §6, §9.5 L8–L9).
> From this amendment on, a plan line that comes from a user decision cites it (a G/Q id, or a ledger id), and a
> line the writer adds says "proposed". Older text is not re-tagged. The known drifts are in the run's audit
> (`workers/w95/report.md`, run `20261022-0805-take-over-5-sessions`).
> Amended again on 10-23 with the user's answers to that audit: the plan restores the grill's decisions where
> they live, and Stage 6 (S28–S37) builds them. A changed line cites the user or the Delegator's judgement;
> all 20 answers are decided (§9.6), including the four defaults settled on 10-23.
> Amended again on 10-23 with the user's D22 (S32: model-fit rows and Refresh) and D23 (S38: the account
> auto-switch order and the provider fallback).
> Amended again on 10-24 with the user's D51 (S39: `brigadier why <file>:<line>`).
> Amended again on 10-24 with run `20261024-1334-long-session-memory`'s run2-D2…D17: long sessions compact in
> place behind `BRIGADIER_LONG_SESSIONS`, off by default until the test round passes (G12, S37, §9.5).
> Amended again on 10-24 with the same run's run2-D23…D43: every session and Chat is an orchestrator, like the
> Delegator; workers do the real work (§1, Teams, §6, S15). Old-run D43 ("Default: do it yourself") is superseded
> by run2-D23; run2-D25 and run2-D30 (the session's own subagents) by run2-D43.
> **Nothing is migrated.** There are no real users yet: existing records, saved settings and open sessions are
> not converted or kept readable. Sessions open at upgrade time are archived.
> Base: origin/main `ae56e9271`. Paths: `M/` = `crates/core/src/manager/`, `P/` = `crates/providers/src/`,
> `D/` = `apps/desktop/src/`. File:line references are to `ae56e9271`. Line counts are rough (`wc -l`, tests
> included unless said).

## 0. Why

On 10-22 all five sessions got stuck (DIAGNOSIS.md). None of the causes was the model. They were all engine
machinery wrapped around it:

- A merge gate (`run_checks` in Brigadier's sandbox) that could not pass for 11 hours. Four "merge anyway?" asks
  reached the user.
- A machine-wide build lease. It queued 2-second checks for 5–10 minutes, about 77 minutes in 95.
- Engine-started turns. In S4, 64 of 66 turns came from engine events, and 24 of them said only `[quiet]`. That
  was 22% of the session's tokens.
- Briefings in place of context. The R5 review findings vanished into a session that died in 387 ms.
- Cards computed from guesses: "Ready" while the thread asked a question, and a red ✗ on "Merged, pushed, CI
  passes".

The user's ask: Brigadier sessions should work exactly like plain Claude Code and Codex conversations.

## 1. The target, on one page

**A session is a plain CLI.** Each tab runs one Claude Code (`claude -p` stream-json) or Codex (`codex
app-server`) session, in the project's worktree or its Local checkout. The session is an orchestrator, like the
Delegator (run2-D23): it talks with the user, plans, looks things up, makes small edits and runs `git merge`,
`git push`, `gh` and CI as plain commands; builds, tests, research and bigger work go to its workers, and it
checks their evidence (run2-D24, run2-D43). Reviews run on a routed reviewer worker, not a hard-coded `codex
review` (run2-D35, run2-D36). Its commands show as ordinary tool rows with their real output and exit code. Its
background tasks, permission prompts and memory files are the CLI's own; its CLI's own subagents are off, a
worker's stay on (run2-D43).

**A session gets exactly what the plain CLI gets (G17).** The project's instructions (CLAUDE.md, AGENTS.md),
the user's MCP servers, skills, and memory as the user's own settings have it. Brigadier strips none of it by
default. It trims only its own additions: the short standing instructions (§6), no MCP server of its own, the
one `brigadier --help` pointer, and the always-installed `/brigade` skill, which costs only its description
until used (G20). The one thing it turns off is the few CLI tools that cannot work inside a Brigadier tab (S17),
never anything of the user's, and, for sessions and Chats only, the CLI's own subagent tool, through the CLI's
own switch (Claude's `--disallowedTools`, Codex's `agents.enabled=false`) (run2-D43, user, 2026-10-24: "we
should disabled subagents for the orchestrator and only allow them for the workers"). Stripping comes back only with measured evidence on our own tasks (§8, item 13).
The user's `disableAllHooks` applies inside Brigadier too, including to Brigadier's own hooks (S37, §8 item 17)
(Delegator's judgement, 2026-10-23: parity wins; Brigadier stops overriding disableAllHooks).
The one exception is a research worker the thread starts with `brigadier start --lean` (G23, S13): the thread's
choice per worker, never the user's own sessions.

D63: every Claude launch uses the CLI’s `--exclude-dynamic-system-prompt-sections`; it and the cache TTL option below are the CLI’s own options, and the facts the model sees are unchanged (G17).

**Brigadier pushes nothing (G1).** A thread receives only:
- the user's messages; one sent while it works comes at once under a one-line frame naming its queue id, to
  judge it (run2-D40, "Deciding" below);
- results of things it started itself, which the CLI already delivers;
- one message per seam: "write your hand-off" (G12);
- its workers' events: through the `brigadier wait` it keeps running, else one batched message once its turn
  ended and no `wait` runs (S29, Q9);
- a message another session chose to send it, when the user asked for it or it was truly needed (S30) (user,
  2026-10-23: "Sessions should be able to message eachother").

There are no status nags, no cross-session notices, no mid-task rule changes, no `[quiet]` and no `[next:]`.

**The UI reads the CLI and git, never the thread's tool calls (G2, G11).**

- **Thread.** The thread is the CLI's own session file: Claude JSONL, or the Codex rollout. While the CLI runs,
  the UI follows its live stream. Brigadier keeps no copy of the conversation. The UI shows the same items in
  the same order as the CLI. It may draw them more nicely, but it never shows different data, and it never
  hides or adds anything. Amendment (run2-D44, user, 2026-10-24: "all this should happen under the hood, user
  shouldnt be bothered! Neither should brigadier message bubbles be visible in the conversation"): Brigadier's
  own frame text is hidden, and a queued message shows once (Deciding, below).
- **Work block.** A turn runs from the user's message to the CLI's turn-end event. Its steps fold into "Worked
  for Xm", timed from the CLI's own timestamps. The turn's last assistant text shows outside the fold. Stopped,
  failed and interrupted come from the CLI's live result event. The session file doesn't hold them, so
  Brigadier writes that one fact per turn to `record/turns.jsonl` (S7).
- **Long sessions.** One thread is the chain of its lineage's CLI session files.

**The `brigadier` CLI is pull-only (G3, G6, G16).** Brigadier's whole tool surface is one command,
`brigadier`, built like `cmux`: subcommands, `--json` output, called through the CLI's own Bash tool. It costs
no context until used, and it is the same for Claude, Codex and the `/brigade` skill. One line of the standing
instructions points at `brigadier --help`. The thread calls it when it wants to:
- **Read anything:** sessions, tabs, states, its own workers, in every project, not only the current one (S30)
  (user, 2026-10-23, w95 item 12: every project).
- **Worker commands equal to `dlg`:** start a worker, send, wait for its workers' events, read the last
  message or screen, close.
- **Message other sessions:** send to and wait on another sidebar session, when the user asks or when it is
  absolutely needed (S30) (user, 2026-10-23: "Sessions should be able to message eachother").
- **Judge a queued message:** `brigadier queue <id> join|wait|worker <worker-id>|new-session` (run2-D40).
- **Previews, browser.**
- **Computer use:** not a subcommand; the vendored driver's own tools sit beside `brigadier` and reach every
  session, Chat and worker while Settings › Computer use is on (S35) (user, 2026-10-23: "fix this!").
- **Ask the user as a card.**
- **Models:** `brigadier models`, the live model, usage and guidance view (G18).
- **Brain search** over the project's records (G14).

Every checks, review, land and headline tool goes. So does the Brigadier MCP server. A thin MCP wrapper over
the same commands is a non-goal; it would come only if a client can't run shell, and none can't today. The CLI
should be shippable as a product later, so it is one binary, its subcommand names are stable, and its interface
has no app-internal paths.

**Teams = the Delegator pattern, built in (G3, G19, G20).** The `/brigade` skill (plain, editable, the one skill
Brigadier ships, G19.Q5) plus the `brigadier` worker commands. **Every session and chat is a brigade (G20).** The
skill is always installed, and one line of the standing instructions says the session may run workers through
`brigadier` and points at the skill. No one invokes it and there is no "New brigade" UI. The user talks to the
session as usual. The session works like the Delegator, an orchestrator (run2-D23, user, 2026-10-24: "it works
just like the delegator, how an orchestrator works"; supersedes old-run D43 "Default: do it yourself"):
- It does the talking, asking and planning, quick look-ups, small edits (a few lines in a file or two), git,
  merges, pushes and CI, and checks workers' evidence (`git log`, their reported commands and output, CI). It
  never builds or tests itself; the checks on its integrated tree go to a worker or CI (run2-D24, run2-D43).
- It has no subagents of its own; workers keep theirs (run2-D43, superseding run2-D25 and run2-D30).
- Anything bigger, anything needing a build or tests, and research beyond a quick look-up go to workers;
  research to a `--lean` worker on the routing's search row ("Mechanical changes and search", or the user's own
  research row) (run2-D24, run2-D36, run2-D43; the row choice is proposed: the shipped rows name no research
  kind).
- Up to N workers at once on disjoint parts, N the user's "Workers at once" (Settings › Configuration ›
  Workers, default 3), read through `brigadier models`; Brigadier refuses no further start (run2-D31, G18).
- It assigns work deliberately, sends a user's follow-up to the worker that owns that area, and reuses a
  finished, unclosed worker (`brigadier send`) while its context is useful and not near full, which `brigadier
  workers` shows; a closed worker is not resumed (run2-D38, run2-D42).
- Grilling is part of the skill: before a big or unclear job it first answers what the code or a web search can,
  then asks the user all it can at once on one `brigadier ask` card, each question with a recommended answer,
  a next round only for questions that hang on those answers (the user, 2026-10-25: never one card per
  question; this supersedes "one question at a time"), and says the shared understanding back
  before work starts. It answers its workers' questions itself when the code or a web search settle them, else
  asks the user; the user always decides spending money, a push or publish they didn't ask for, credentials,
  and changes to their settled decisions (run2-D32; the always-ask list is proposed, from the Delegator's
  rules). A Chat's and side chat's own grilling line points to the skill's: all at once on one card, no
  numbered rounds (run2-D32).
- After a block of work it replies in at most 3–5 short lines: what changed, what's next, what needs the user;
  workers keep full reports, which only the session reads (run2-D27).
Every session and normal chat brigades, with no option; side chats stay as they are, with no workers (run2-D26).

**A message sent while the session works: Deciding (run2-D40).** Back as it was in #437 (027893857, 7a3114dbd),
rebuilt on the session's own `brigadier` command: no MCP server and no tool server is involved (the old judgment
went through Brigadier's MCP tool and stopped working when S16c removed that server) (user, 2026-10-24: "sure,
do it"). A message the user sends while the session's turn runs (queueing on, the queue not paused):
- **Rail.** It waits in the queue rail with the old Deciding look: a spinner for the glyph, a shimmering
  "Deciding", and the held note "Read when the current command ends" while a command of the thread's runs
  (run2-D40). The rail reads the queue item's state, which only the daemon writes (G2).
- **Delivery.** It reaches the session at once as the user's own message (G1), under one line,
  `[queued message <id> · judge it: brigadier queue <id> join|wait|worker <id>|new-session; unjudged, it joins
  this work]`: Claude's stream-json user message, taken at its next step; Codex's `turn/steer`, which never
  starts a turn for it. With no Codex turn running to take it, it waits plainly (run2-D40, run2-D46).
- **Judgment.** The session answers with `brigadier queue <id> …` (run2-D40):
  - `join`: it is the user's message of a request of its own inside the running turn, never sent again;
  - `wait`: it stays queued and goes on its own turn once nothing works, under
    `[queued message <id>, now on its own turn]`; the earlier copy is hidden in the thread (proposed: resent
    rather than a bare pointer, so a compaction can't lose it);
  - `worker <worker-id>`: Brigadier gives its words to that worker of the session's, as `brigadier send` does
    (run2-D38), and the rail keeps a one-line "Sent to worker #N" notice the user dismisses (proposed:
    dismiss-only; attachments go as their paths);
  - `new-session`: it waits with "Open as a new session" and "Run here", and is never sent until the user picks.
  - Re-judging a message still queued is allowed (proposed). A worker can't judge; it has no queue.
- **Unjudged (the earlier default, run2-D40).** After 20 s it joins the working answer; still unjudged when its
  turn completes, it joins that turn; after a stop, a failure, a limit or a daemon restart it waits for its own
  turn. A late judgment hears it already joined.
- **Edit and delete (proposed).** Once the session has it, the rail can't edit it; deleting it tells the session
  `[queued message <id> withdrawn: the user took it back; leave it]`, and its text stays in the thread where the
  session read it.
- **Thread (run2-D44).** The frame lines and the withdrawal line are never drawn. A queued message shows once:
  in the rail while the session has it there, else where the CLI last took it.
- **Who gets it.** Sessions and normal Chats (proposed: run2-D46, from run2-D26); side chats and tab chats keep
  the plain queue. A Steer, or queueing turned off, still sends it straight in.
Overnight is the same skill with a deadline. The engine has no lead, phase or landing protocol.
A Chat is a brigade too and acts almost exactly as a normal session: it starts workers, and its CLI has its own
shell with the Chat's session folder as cwd, with no `brigadier`-only gate (S28) (user, 2026-10-23: "a chat is
always a brigade").

**A worker belongs to its parent session (G19).** A worker is never a sidebar session. `brigadier start`
creates a worker of the calling session, and it lives only inside that session: in its Workers row (context
card), the rail above its composer and its worker cards and panel. It is like Claude Code's subagents or the
Delegator's worker tabs. Archiving or deleting the parent takes its workers (S20). Under the hood a worker is
still a plain CLI with its own worktree and its own checks. A worker can't start workers of its own; it may use
its CLI's own subagents (user, 2026-10-23: "workers cant start other workers").

D63: Claude workers (including lean research and fusion consults) and Brigadier’s one-shots use `CLAUDE_CODE_PROMPT_CACHE_TTL=5m`; user sessions, Chats and their seam successors keep the CLI default.

**The parent knows everything about its workers (Q9).** A worker's turn end, question, prompt or stop reaches its
parent, idle or not, the Delegator way: a `brigadier wait` the parent keeps in its CLI's background re-invokes it
(S29) (user, 2026-10-23: "The parent should know everything").

**Workers land into their parent (G21).** A worker branches from its parent session's branch (`brigadier start
--base` defaults to the parent's current branch) and merges back into the parent when its step is done. The
parent decides merge or rebase and resolves conflicts, with the worker's help. The parent integrates, then merges
into what the user chose: the New worktree's base branch, or the Local checkout's branch chosen from the rail.
Brigadier enforces none of this; the `/brigade` skill guides it. Nothing lands on main by rule.

**Workers are read-only to the user (G19.Q6).** The user can open a worker from the rail and read its thread.
There is no worker composer. The parent reports what a worker needs: Waiting on you lists the worker's real
pending prompts and questions. The user answers in the parent, and the parent relays the answer with
`brigadier send`. A worker's permission prompt is answered the same way, with `brigadier send <worker>
--allow|--deny` (Q8). An `@worker` mention in the parent's composer is only a routing hint to the parent. A second
composer would duplicate the parent and complicate the UI.

**Model choice is advice, not a gate (G18).** The thread picks provider, model and effort itself, through the
`/brigade` skill. Before `brigadier start` it reads `brigadier models`: what each CLI reports as available, usage and
reset times per account, the current account, a shipped guidance table, and the user's own rules on top
(S13b). **Brigadier enforces nothing on its own.** `brigadier start` refuses only what the user's rules file
forbids. The shipped default of that file is neutral and forbids nothing; the user's own rules (for this user:
never Fable, effort ≤ high) live in the user's own file (S33) (user, 2026-10-23, w95 item 16).
Settings › Routing is the editor of that same file, with the user's preferences; `brigadier start` and the
skill pass the model and effort those rules name (S32) (user, 2026-10-23: "user should've been able to apply
his rules"). Every worker, reviewer and fusion consult the session starts takes the whole routing choice
(provider, model and effort) of its Work's row; out of quota, it takes the same Work's next row the rules
allow, all three fields kept (proposed), never a free override (run2-D36, run2-D39). A worker's own subagents
are held to the rules by the existing allowlist (`M/models.rs`). The old task router and its task rerouting go. Account auto-switch stays, ordered smartly, and
"Switch to another agent automatically" comes back: when every account of a provider is out, a running session
seams to another CLI (S38) (user, 2026-10-23, D23: "if all the claude quota is done, it should fallback to
another provider if enabled").

**Permissions pass straight through (G5).** The composer offers Ask, Accept edits and Full access. These map to
Claude Code's and Codex's own modes. The CLI's own permission prompts show as cards. Brigadier runs no sandbox
of its own.

**Context card (G9, G10).** It has these rows:
- **Project and branch**, with +/− from git.
- **PR row:** `#N`, a state word and a colour, from `gh`. It has no actions. The words and colours are the
  grill's list: Draft (grey), Checks running (yellow), Review needed (yellow), Ready (green), Merged (purple),
  Closed (grey), Checks failed (red). It refreshes after `git push` and `gh pr`, plus a light poll while the
  session is open; without a `gh` sign-in it says "gh auth login for PR status" (S34) (user, 2026-10-23: "fix
  this").
- **Waiting on you:** only real pending CLI permission prompts, ask-user questions and MCP sign-ins of this
  session's workers. The user answers them here, and the parent relays (G19.Q6). This session's own question
  shows only above the composer.
- **Workers:** this session's own workers (G19). Title, state from the worker's own CLI events, +/− of its
  worktree against the parent's branch (G21), and "N done · M stopped". The rail above the composer and the
  worker cards read the same.
- **Background:** the CLI's own background tasks. The label is the CLI's description, with elapsed time. These
  rows behave exactly like Claude Code's own.
- **Sources:** attachments first, then fetched URLs, then MCP servers used; + adds one.

No context-used number shows anywhere, except the context ring in a Chat's composer, which stays as it is
(user, 2026-10-23: "ok"). A usage-limit "Resumes at" card is allowed; it reads the reset time from
the CLI's own error. The Job card is gone, overnight included (G7).

**Changes card (G8).** Brigadier takes a git snapshot of the session's own worktree at each turn's start and
end. The card shows the exact diff between the two, minus files that a merge of another branch brought in: what a
worker's merge brought into the parent shows under that worker's card, not as the parent's own change. Revert
restores the start snapshot. A worker's card shows its own +/− against the parent's branch (G21).

**Long sessions (G12).** Two designs sit behind one switch, `BRIGADIER_LONG_SESSIONS`. It stays off by default
until the test round passes (run2-D15). Off, the seam design (switch off, below) is what ships. On, the CLI
compacts in place (switch on, below; run2-D2).
- **Ledger.** Both designs keep it. The thread keeps `record/ledger.md`: one entry per thing settled (a rule
  or decision, an open item opened or closed), batched with its other tool calls, never a progress log, and none for a task that settled
  nothing. A standing instruction asks for it; nothing enforces it (Q3). (The "never a progress log" wording
  came from the w123 bench, 10-23: Codex wrote 3.6 entries per small task, +26% wall against plain.)
  - `brigadier ledger add` gives each entry an id and the time. Rules and decisions quote the user. Open items
    (promises, questions waiting on the user) are one kind, closed by id. At every seam they travel first and
    whole (S22b, `research/CONTINUITY.md`).
- **Switch off (the default, what ships today): seams.** The CLI never compacts.
  - **Nudge.** Past 200k tokens of hot context, Brigadier queues one mid-turn nudge, Delegator style. 200k is
    an env default, not a cap (Q1, Q2). The thread finishes the step it is on, at a boundary it chooses, then
    updates the ledger and writes a hand-off note. The hand-off instructions travel in that nudge, not in the
    standing instructions (§6).
  - **Successor.** A fresh CLI session continues in the same tab, after a visible divider: "Fresh context ·
    hand-off note", and the note opens from it. It starts at once and carries on from the note by itself, as a
    Delegator's successor does; it never waits for the user's next message (S22, S23) (user, 2026-10-23: "Why
    wait? See how delegator works"). Its bootstrap is the first lines of its first user message, also
    saved to disk (Q4). The successor may search every earlier file of the lineage (Q5).
  - **Codex.** Codex's forced compaction at 90% is a known limit. The divider shows when it happens (Q0).
- **Switch on: the CLI compacts in place (run2-D2).** Behind `BRIGADIER_LONG_SESSIONS`, off by default, until
  the test round passes (run2-D15). The tab keeps one CLI session for its whole life. What each lever did on
  the installed CLIs: `docs/evidence/2026-10-24-long-session-levers.md`.
  - **The switch.** Read when the daemon starts. Its value is the mode, one of the battery's candidate arms
    (`tools/continuity-battery/arms.py`): `native-bare` adds nothing; `native` or `restore-only` puts the
    decisions back; `instructions-only` steers Claude's `/compact`; `native-full` does both;
    `native+checkpoint` and `native-full+checkpoint` add the checkpoint. An empty or unknown value is off. It
    applies to a session's thread on Claude and Codex only; everything else keeps seams (run2-D13).
  - **Codex (P1, run2-D2, run2-D6).** No size seams. The 872k window override goes. Brigadier sets
    `model_auto_compact_token_limit` only when `BRIGADIER_SEAM_TOKENS` is set explicitly (run2-D16), and a
    user's own `model_auto_compact_token_limit` wins (G17). Codex compacts by itself at its own point (≈245k
    on a 272k-window model). The battery's candidate arms use a shared point (`--tokens`, 200k by
    default), except the pilot's P1 Codex long `@codex-default` variants, which omit the override and
    test Codex's own point (run2-D18). Brigadier reads the compaction from the
    completed `contextCompaction` item. The thread shows the CLI's own "Compacted" divider.
  - **Claude (P2, run2-D5, run2-D7, run2-D12).** At a turn end past the point (≈200k), Brigadier's next turn
    is `/compact` with keep-instructions: open items; decisions in the user's words with their ledger ids;
    failed commands with their error lines; checked claims; files changed; the last test and its command; the
    next step; skills in use; exact evidence pointers, `brigadier brain search/read` ids included; never a
    secret (D71). Only the `instructions` arms send them.
    - No PreCompact refuse hook and no `DISABLE_AUTO_COMPACT`.
    - `CLAUDE_CODE_AUTO_COMPACT_WINDOW` is the point + 50k: the backstop for a turn that runs on. Claude
      compacts ≈20k below its window, so at ≈230k (**extrapolated** from a 100k-window measurement; not run
      at 250k).
    - A user's own window or `autoCompactEnabled: false` wins: Brigadier sets no backstop, and its own
      `/compact` at the point still runs (G17, run2-D16).
    - `idleCompaction: false` stays (run2-D12).
  - **Decisions put back whole (run2-D8, S22b).** After every compaction, the ledger's open items, rules and
    decisions go back word for word.
    - **Claude:** 8 `SessionStart` hooks on `compact` (`brigadierd hook restore --slot k`). Each prints one
      labelled slice of at most 9,500 characters, plus a lookup line (`brigadier brain search/read`).
    - **Codex:** one `thread/inject_items` note, marked `brigadier-restore <compaction-id> <fingerprint>`.
    - **Receipts.** Brigadier reads the CLI's own file: Claude's hook attachments after the
      `compact_boundary`, the Codex rollout's note naming the compaction. Every active entry must arrive whole,
      exactly once: the entries that went back after that compaction, not a decision written since. The state
      is kept in `record/long.json`, so a retry or a restart checks the file instead of sending again (tested:
      answer lost, receipt lost, restart).
    - **What the user sees:** the divider only. A notice shows only on failure, naming why (D13 of the
      take-over run, run2-D16). With Claude's hooks off in the user's settings, a one-time notice says so
      (run2-D16; an exception to S37's silent degradation). The ledger is unchanged (D28: append-only).
  - **Checkpoint (P2b, run2-D9), in the `+checkpoint` arms.** `brigadier checkpoint set <text>` and
    `brigadier checkpoint show`. A separate, replaceable record (`record/checkpoint.md`, history in
    `checkpoints.jsonl`), not the ledger (D28), at most 8,000 characters. It holds the goal; open requests with
    their Brain item ids and the exact `brigadier brain read <session> --item <id>`; important command results
    by item id; files changed; the last test and its command; the next step; skills in use. It needs the
    Brain's read by id (P7).
    - **Codex:** past the point (≈200k, so before Codex's own; battery arms only: compacting before
      run2-D6's point needs the user's yes before it ships), Brigadier asks for it in a turn of its own,
      hidden in the thread like `/compact`, then calls `thread/compact/start`. No user message joins the ask or slips in between: it waits
      until the compaction ends. A checkpoint written goes back inside the decisions note, and the receipt
      checks it whole. None written: Codex compacts anyway, and the checkpoint is marked missing. Codex
      compacting by itself during the ask calls Brigadier's compaction off. The run is kept in
      `record/long.json`, so a restart goes on: asked → compact with what was written, never asking again;
      compacting → compact again.
    - **Claude:** the steered `/compact` asks the summary to end with a "## Checkpoint" section. Brigadier
      keeps that section as the record. A compaction Claude starts by itself marks it missing.
    - Codex's own token_budget, history_notes and new_context stay off.
  - **Fallback seam (run2-D10, P3).** A fresh session starts only when:
    - compaction fails;
    - Claude stops with "Autocompact is thrashing";
    - the decisions can't be put back whole;
    - the context stays above half the point after compacting (**proposed**; the threshold waits on the test
      round). Claude only for now: Codex reports no size after compacting.

    On `prompt_too_long`, the CLI compacts first and the failed request goes again once, by itself. A second
    failure starts a fresh session that carries the request. The seam's hand-off gets P3's repairs: the
    lookup instruction, tool-evidence pointers and the working state. Moves to another CLI (D46) and quota
    fallback across CLIs (D23, D25) still use the seam. Account and model switches resume the same session
    (D14).
  - **Not built: a higher Codex output cap (P5, run2-D11, run2-D17).** On Codex 0.164.0,
    `tool_output_token_limit` changed nothing on gpt-6-luna with code-mode `exec`. Revisit when a Codex
    version honours it.

**Brain (G14).** The Brain is a pull-only search over the project's records:
- every session's ledger;
- the CLI transcripts;
- hand-off notes;
- "learned from reading" pointers.

Briefings, context packs, the Brain-first rule, the rebirth Brain budget, engine-written learned nodes and
enrichment jobs all go. The code index stays as an optional command. The Personal Brain is deleted outright;
the CLIs' own memory (Claude Code's auto memory, Codex memories) is no longer switched off.

**Folders (G13, G15).** Each session gets one folder, `~/.brigadier/s/<short-id>/`:

| Subfolder | Holds | Archive | Delete |
|---|---|---|---|
| `record/` | ledger, hand-off notes, `lineage.json`, `turns.jsonl`, `seam.json`, `workspace.json` | keeps | removes |
| `work/` | the worktree | removes once merged | removes |
| `tmp/` | `TMPDIR` points here | removes | removes |
| `dev/` | dev servers and build output | removes | removes |
| `logs/` | logs | removes | removes |

`TMPDIR` points at `tmp/` as a convention, not a guard. A worker gets its own folder of the same shape, owned
by its parent. `tmp/` is a plain `TMPDIR`: nothing empties it between turns and no process is killed; only
Archive and Delete clean it (S31) (user, 2026-10-23, w95 item 13: the grill's design stands).
- **Archive** also stops the session's processes, and keeps its Brain entries and lineage transcripts.
- **Delete** removes everything of the session: its folder, its Brain entries, its lineage's CLI transcript
  files, its worktree, and its branch after the existing "not merged" ask.
- Both do the same to each of the session's workers (G19).
- **Storage** shows the size of each folder, and its cleanup points out worktrees that are merged, have no
  commits or are idle (S20). It is guidance; a dirty worktree still gets the "not merged" ask.

Some things stay where they are: the CLIs' own session files, shared caches (`~/.cargo`, the pnpm store) and
cross-session locks.

**Local checkout stays** next to New worktree. New worktree remains the default (built 10-22, `2d72ce20f`).

## 2. What this supersedes

In `docs/PLAN.md`:

- **§1.** "The orchestrator never does work itself", and these Design-goal rows:
  - Context: "Brain + invisible rebirth"
  - Review: "risk-tiered, cross-vendor"
  - Security: "OS sandbox for workers"
  - Routing and Fallback, as far as they route tasks to workers
- **§2.** These principles:
  - 1, "The orchestrator only talks"
  - 2, "Brigadier owns the truth": the CLI's session file is now the truth
  - 4, a verifier per phase
  - 9, as far as it removes CLI session files: Archive keeps them now
- **§3.** The architecture box items Orchestrator runtime, Worker pool, Router, Review engine, "Sandbox" and
  "Brigadier MCP server" (now the `brigadier` CLI, G16). Also the line that Brigadier stores the canonical
  transcript.
- **§5.**
  - **Session:** "Brigadier's git engine performs merges"; workers' temporary worktrees.
  - **Lifecycle:** Archive's and Delete's lists, replaced by G15 (§1 above). Delete no longer keeps what the
    Brain learned. This reverses Q25.
  - **Permission level:** the three levels and their sandbox, replaced by G5.
  - **Cleanup ledger:** removing CLI transcripts on archive.
  - **Task, Worker, Artifact and Personal Brain.**
- **§6.**
  - **Phase 3:** the orchestrator runtime, its MCP tool list, "A phase lands once…", the permission levels and
    the engine feed of worker cards. The cards themselves stay (G19).
  - **Phase 4:** rebirth, briefings, skeleton pass, idle-quota enrichment and Personal Brain.
  - **Phase 5:** mid-task cross-provider hand-off of a task.
  - **Phase 6:** verifier, verification pipeline as an engine step, freshness research scout.
- **§7.** The rebirth and hand-off paragraph. The rules stay: no caps, lossless, no settings, judged per
  completed task.
- **§9.** These rows:
  - Q2, Q5 (mid-task fallback of a task), Q6, Q8, Q13 (Personal Brain), Q15, Q18 (enrichment), Q25 (Delete
    keeps Brain), Q26 and Q27
  - the 2026-10-05 "Delegator-style flow" row
- **§10.** Overnight mode, all of it. Overnight is now the `/brigade` skill with a deadline.

Other plans lose the parts G1–G21 change. Where they conflict, this plan wins:
- `THREAD-PLAN.md`: engine tools, reviews and the verifier.
- `THREAD-UX-PLAN.md` and `THREAD-PARITY-PLAN.md`: thread items from engine records, and the Job card.
- `SESSION-MESSAGING-PLAN.md`: peer tools become the `brigadier` worker commands of S13, parent to its own
  workers only, and session-to-session messages through `brigadier` (S30) (user, 2026-10-23: "Sessions should be
  able to message eachother").
- `WORKSPACES-PLAN.md`: auto-merge.
- `SESSION-FIXES-PLAN.md`: merge gates.
- `docs/research/long-sessions/BUILD-PLAN.md`: rebirth, briefings and the output store.

w03's "Job card only for plans and overnight runs" (`4b1ea4c2a`) is a stopgap until S16.

## 3. Deletion inventory

Each part below goes. The files, rough sizes and closing issues are listed with it. "Closes" means the open
GitHub issue becomes obsolete when that part goes; the step closes it with a note. The total is roughly
**55–65k lines of Rust plus about 4k of TypeScript**, about half of it tests. This is an estimate from the
inventory.

### 3.1 Engine pushes into the thread (G1) → step S1
- `[status due]`: `M/status.rs` (225 lines; text at :134, 30-minute timer at :45–66).
- `[quiet]`:
  - the constant at `M/prompts.rs:214`, and `late_event!` at :397–406;
  - in `M/conversation.rs`, the Quiet filter at :7069–7171, `waits_quietly` at :7277 and the deferral at
    :5536–5560 and :5761.
  - `M/quiet.rs` is unrelated (DB maintenance). It stays.
- Cross-session notices:
  - `M/project_sessions.rs` `tell_overlaps` at :486 ("[project] … also changed") and `tell_merged` at :553;
  - `hold_fact` at `M/conversation.rs:1224`.
- "From now on" rule revisions and `[settings]` notes:
  - in `M/prompts.rs`: `REVISIONS` at :800–1363 (about 560 lines) and `notes()` at :2115–2535;
  - in `M/conversation.rs`: `instruction_notes` at :6612 and `told_notes` at :6677;
  - in `M/workers.rs`: the worker revisions at :1854.
- `[next: …]` hints: `M/prompts.rs:410–420` and `M/conversation.rs:7123–7170`, plus `keep_guess` at :5287.
- Engine-event turns:
  - `deliver`, `kick` and `next_turn` at `M/conversation.rs:2704–3363` shrink to user messages, plus results of
    work the thread started (until S16) and the seam nudge.
  - `M/inbox_store.rs` (443) narrows the same way.
- Tests: `flow/status_tests`, `quiet_wake_tests`, `settings_toggles_tests`, `suggestion_tests`, and parts of
  `project_sessions_tests`, `owed_tests` and `restart_waits_tests`.
- Closes: #520 (with S4).

### 3.2 Check gates, the full-checks sandbox and the build lease (G4, G6) → step S4
- **Tools and checks code:**
  - Tools: `run_checks` (thread and worker) and `run_check`.
  - `M/integrated_checks.rs` (1,778): `full_check_access` at :783 and the checkout at :725.
  - `M/checks.rs` (3,939).
  - `M/test_counts.rs` (321): its one caller, `preview.rs:1158`, loses the count.
- **The full-checks sandbox test machinery** (added by `54eba0403`, #493):
  - `Nested`, `test_platform`'s wrapper and `skip_inside_a_sandbox`, at `M/flow/mod.rs:1233–1302`.
  - The skips at `M/preview.rs:3359`, `M/protection.rs:244/403`, `M/run.rs:753` and `M/workers.rs:6474`.
  - `no_terminal_here()` and its 9 uses in `crates/daemon/src/terminals.rs:868–1344`.
  - `nested_sandbox_refused` at `crates/sandbox/src/macos.rs:946`, and `confined()` (`crates/sandbox/src/lib.rs`,
    `macos.rs`), unless S6 still needs it.
  - The test `the_core_tests_pass_in_the_full_checks_sandbox` at `flow/integrated_checks_tests.rs:478–570`.
  - The CI job **"Core tests in the full checks' sandbox (macOS)"** (`checks-sandbox`, `.github/workflows/ci.yml`
    after the `rust-tests` job), and the header comment that names it.
- **The build lease:**
  - `crates/core/src/machine/builds.rs` (1,282; "One build at a time") and `machine/heavy.rs` (455).
  - The lease rows and held workers in `M/machine.rs` (234).
  - The lease calls in `checks.rs:1212–1250` and `preview.rs:123/899–923/1596–1693`.
  - The heat and memory guard in `machine/mod.rs` stays until S16 decides, because workers still use it.
- Tests: `integrated_checks_tests` (570), `checks_tests` (533), `commit_on_pass_tests` (293), the
  `run_checks` cases in `agent_tools_tests`, the lease rows in `background_tests`, and the inline tests of
  `builds.rs`, `heavy.rs` and `checks.rs`.
- Closes: #514, #515, #479. Re-check #382 and #296 after S16 (most flow tests named in them go).

### 3.3 Reviews, merge gates and "merge anyway" (G4, G6) → step S5
- **Review tools:** `request_review` (thread and worker), `record_findings` and `review_plan`.
  - `M/review_runs.rs` (2,713): the findings ledger, the "Review the work" setting and `auto_merge_on`.
  - `crates/review` (1,160), `M/fusion.rs` (290) and `M/unreviewed.rs` (243).
- **Merge gates:**
  - `merge_rule` at `M/landing.rs:2116–2164` ("needs run_checks passed"), plus `checks_waived` and the consent
    functions at :2170–2508.
  - `M/merge_consent.rs` (130), `M/review_hold.rs` (366), and `M/merge_record.rs` (396, which `pull_request.rs`
    also reads).
  - `MergeSession { anyway }` (`crates/daemon/src/server.rs:757`).
  - `MergeRefused`: `work.rs:1928`, `board.rs:214/613`, `model.rs:1495`, `sessions.rs:1532` and
    `ipc/src/protocol.rs:911`.
- **UI:** the "Merge anyway" rows in `D/app/conversation/Activities.tsx` and `backgroundRows.ts`.
- **Tests:**
  - `held_merge`, `merge_by_user`, `merge_queue`, `auto_merge`, `review_hold`, `review_tiers`, `review_switch`,
    `review_picks`, `findings`, `fusion`, `unreported` and `gate_fixes`;
  - `agent_tools_tests:947–1086`.
- Closes: #519, #518 for its card half. The CI half was fixed by `8bce22754`: check the last Windows Clippy runs
  and close.

### 3.4 Brigadier's sandbox and its permission levels (G5) → step S6
- Sandboxing of CLIs, runs and previews:
  - `providers/src/cli.rs:259–287` wraps every CLI in the "protect" profile, Full access included.
  - Seatbelt `confine`, `protect` and `confine_preview` at `crates/sandbox/src/macos.rs:614–920`, plus the
    matching parts of `linux.rs` and `windows.rs`.
  - `M/protection.rs` (573).
  - The separate confines in `M/run.rs:233/354`, `M/preview.rs:850` and `M/workers.rs:6498`.
- Brigadier's own reviewer for unsafe commands:
  - `M/escalation.rs` (408);
  - the Codex auto-review answering in `P/codex/mod.rs:1149–1288`.
- Tools: `run` and `run_unsandboxed` (`M/run.rs`, 838) and `suggest_full_access`.
- In the Claude `--settings`: the `sandbox` block, `disableBypassPermissionsMode`, `autoMode.hard_deny`, and the
  ask rule on `Bash(dangerouslyDisableSandbox:true)` (`P/claude/mod.rs:656–946`).
- UI: `D/.../SandboxLimitNotice.tsx` (86), and the "Approve for me" wording in `SetupPickers.tsx`.
- The rest of `crates/sandbox` stays: paths, processes, Keychain, login shell and machine.
- Closes: #521.

### 3.5 Phases, outlines, overnight and the Job card (G4, G7) → step S16a
- **Tools:** `plan_phases`, `propose_plan`, `approve_outline`, `start_verifier`, `submit_outline`,
  `propose_overnight`, `settle_step` and `end_run`.
- **Engine:**
  - `M/phases.rs` (1,160).
  - `M/overnight/*` (about 7.6k), `crates/core/src/overnight.rs` (628), and in the daemon
    `overnight_supervisor.rs`, `overnight_notifications.rs` and the overnight bits of `awake.rs` and `idle.rs`.
  - `M/decisions.rs` (1,822), `M/constraints.rs` (476) and `cards.rs` `decide_plan` (:670).
- **Job card, engine side:**
  - `set_headline` at `M/tools.rs:1112–1146`, and `HEADLINE_RULE` at `prompts.rs:465`;
  - `HeadlineSet` (`model.rs:2547`) and `SessionHeadline` (`work.rs:1906`).
- **Job card, UI** (about 1.9k lines):
  - `D/app/conversation/cards/JobCard.tsx`, `JobStack.tsx`, `liveJob.ts`, `boardJob.ts`, `runJob.ts` and
    `job.ts`;
  - `components/conversation/elements/job-progress.tsx` and `app/side-panel/JobTab.tsx`;
  - the plan cards in `cards/` (`PlanCard`, `PlanDocument`, `PlanFrame`, `PhaseWork`, `PhaseMore`, `DoneWhen`,
    `PlanRating` and friends).
- **Tests:** `plan_tests`, `phase_acceptance_tests`, `overnight_tests`, `engine_tests`, `constraints_tests`,
  `decision_index_tests`, `card_tests`, `stop_hook_tests`, and the headline cases in `agent_tools_tests`.
- WaitCard is replaced by the "Resumes at" card read from the CLI's error (S14).

### 3.6 Engine-side worker orchestration, routing and the landing engine (G3, G4, G19) → steps S16b, S16c
What goes is the engine's orchestration: spawning, routing, holding and merging workers, the hand-off and resume
machinery, and context packs. **The worker UI stays (G19):** the rail above the composer
(`activity/WorkersStrip.tsx`, `StripRow.tsx`, `stripView.ts`), the Workers row (`side-panel/WorkerSummary.tsx`)
and the worker cards and panel (`side-panel/WorkerList.tsx`, `WorkerThread.tsx`, `WorkerHeader.tsx`,
`WorkerChip.tsx`, `WorkerActivity.tsx`). S14 re-feeds them from the worker's own CLI events and worktree git;
only their engine-record reads go. The parent → worker link stays as well, moved into the session store.
`cards/PhaseWork.tsx` is the Job card's phase list, not worker UI, and goes with §3.5.
- **Worker tools:** `delegate_task`, `message_worker`, `answer_worker`, `stop_worker`, `rerun_task`,
  `route_follow_up`, `list_tasks`, `read_report`, `read_artifact`, `store_output`, `ask_thread`, `note_progress`,
  `submit_report` and `attach_proof`.
- **Landing and CI tools:** `land_phase`, `finish_session`, `stop_merge`, `ci_status`, `watch_ci`, `clean_up` and
  `note_for_user`.
- **Session and suggestion tools:** `offer_session`, `suggest_setting`, and the peer tools that S13 replaces
  (`message_session`, `wait_for_replies` and `settle_wait`).
- **Worker engine:**
  - `M/workers.rs`, about 6k of its 7,033 lines. Kept and moved: `make_worktree` (:2578), `session_worktree_path`
    and `ensure_target` (:2642–2700), and the temp folders (:5812–5866), until S7 re-homes them.
  - `M/worker_handoff.rs` (994), `M/worker_resume.rs` (303), `M/outcomes.rs` (272) and `M/reruns.rs` (317).
  - `M/research.rs` (894), `M/context_pack.rs` (1,013) and `M/peer_answers.rs` (321).
  - The worker-silence half of `M/watchdog.rs`.
- **Routing:** `M/routing.rs`'s task routing (923), and `crates/router`'s decision code (`decide.rs` 3,868).
  - `ProviderKind` and `Registry` stay.
  - `crates/core/src/routing/*` stays (quota and model lists). It feeds `brigadier models` (S13b).
  - `M/fallback.rs` (946) goes as far as it reroutes a task; account switching stays.
  - Built-in model rules go with the router, such as the Fable exclusion in `M/routing.rs:368/498`. Under G18
    such rules live only in the user's rules file (S13b).
- **Landing engine and CI watch:**
  - `M/landing.rs` (3,477, what S5 left).
  - The CI-watch half of `M/ci.rs` (2,358). The `gh` PR read in `M/pull_request.rs` stays and moves to S14's PR
    row.
  - (`M/background_runs.rs` goes earlier, in S12.)
- **Prompts:** the worker prompts and report envelopes at `M/prompts.rs:2623–3440`, and the protocol half of
  `thread()` at :260–363.
- **Tests:** `resume`, `successor`, `worker_turns`, `watchdog`, the peer test files, `takeover` (worker
  hand-back), `thread_overflow` and `base_merge`; most of `flow/tests.rs` and `thread_tests.rs` are rewritten.
- Closes: #480, #481, #276, #271, #295, and #482 (workers are plain CLIs with their own worktree, branched from
  the parent's branch, and write access; they merge into the parent, G21).
  Re-check #456, #45 and #34. They test scout and stand-in paths that no longer exist; close them if so.

### 3.7 Brigadier's copy of the conversation and its guessed states (G2, G11) → steps S10, S11, S16b, S23
- **Thread items from the engine store:**
  - `M/thread_items.rs` (845) and the `orch:<id>` log written by `log_orchestrator` (`M/conversation.rs:6959`);
  - `M/export.rs` (874), the successor transcript;
  - `D/state/board.ts` fields that only these feed.
- **Request inference:**
  - `M/requests.rs` (1,293) and the worked and waited spans in `work.rs` (`moved_to` :2994);
  - `M/status.rs` if anything is left after S1.
  - The display stops using them in S10. The code goes only with its last consumers (§4, "Last consumers").
- **The changes card's sources:**
  - `M/request_changes.rs` (1,693, the most guess-heavy file), `M/pending_changes.rs` (884),
    `M/shell_changes.rs` (634) and `M/undo.rs` (210);
  - the PreToolUse and PostToolUse capture hooks in `P/claude/mod.rs`.
  - Closes: #256.
- **The sidebar's engine half:** the `Board::activity` and `settled_state` use in `board.rs:348–420` and
  `project_sessions.rs:217`. Git ahead, merged and reverted stays.
- **Archive and Delete removing CLI transcripts:** `ClaudeSession`, `ClaudeProjectDir` and `CodexThread` ledger
  artifacts disposed on archive (`M/lifecycle.rs:1040–1075`). Under G11 and G15 these are the record, so
  Archive must keep them (S8).

### 3.8 Briefings, rebirth and Brain machinery (G12, G14) → steps S18, S19, S23
- **Fork-based rebirth and briefings:**
  - `M/rebirth.rs` (1,879): `HANDOFF_PROMPT` at :92, the 5k Brain budget at :62, and `briefing()` at :590.
  - The rebirth thresholds in `crates/core/src/knowledge.rs:175–406`, including the `BRIGADIER_REBIRTH_TOKENS`,
    `BRIGADIER_COLD_REBIRTH_TOKENS`, `BRIGADIER_HARD_LINE_TOKENS` and `BRIGADIER_BRIEFING_TOKENS` env vars.
  - `M/cold.rs` (407) and `M/conversation/overflow.rs` (412).
  - The recovery and stand-in reseed: `M/conversation.rs:3196–3210`, :5960–6110 and :6469–6530. There are no
    hidden restarts on quota errors any more. `switch_account`, which continues the same session, stays.
  - Tests: `thread_overflow` and `rebirth_words`, plus the rebirth cases.
- **Brain-first and learned nodes:**
  - the Brain-first prompt line (`prompts.rs:313`) and `M/brain_first.rs` (41);
  - in `M/brains.rs`: `learn_report` (:1494), `learn_user_decision` (:1797) and `keep_note_decisions` (:1866);
  - the `remember` tool (`brains.rs:861`), which the ledger replaces.
- **Enrichment jobs:** `M/brain_jobs.rs` skeleton and idle-quota enrichment (about 1,250 of 1,422 lines), and
  `record_job_nodes` (:712).
- **Personal Brain:**
  - `save_memory`, `memory_lines`, `preference_lines` and the preference notes;
  - `list_memories`, `edit_memory` and `forget_memory` (`brains.rs:2172–2275`);
  - `personal.sqlite`;
  - the forced `autoMemoryEnabled:false` (`P/claude/mod.rs:827`) and Codex's forced `memories` off
    (`DISABLED_FEATURES`, `P/codex/mod.rs:148`).
  - Deleted outright: nothing is exported or migrated (user, 10-22).
- **Thread prompt:**
  - `M/prompts.rs` `thread()` and its rule macros. The standing instructions (§6) replace them.
  - `M/instructions.rs` (572), which inlines CLAUDE.md and AGENTS.md that the CLIs already read themselves.

### 3.9 The Brigadier MCP server (G16) → steps S13, S16c
- `crates/mcp-server` (about 3.2k: `lib.rs`, `catalog.rs` 2,687, `schema.rs`), `brigadier_server()` at
  `M/conversation.rs:3868`, the `ToolHost` impl at `M/mod.rs:979–1060`, the call types in
  `crates/core/src/tools.rs` (`OrchestratorCall` :1371, `THREAD_TOOLS` :1432), the dispatch in `M/tools.rs`, the
  MCP aliases and `MCP_TOOL_TIMEOUT` in `P/claude/mod.rs` `session_env` (:413), and Brigadier's entry in each
  session's `--mcp-config`. The user's own MCP servers keep loading.
- What the thread still needs moves to the `brigadier` CLI in S13. The rest goes with the tools it served.

### 3.10 Open issues
- **Closed by a decision:** #517, by G9 (user, 10-22). A thread that asks in plain text and ends its turn reads
  Ready, like an idle plain CLI. Only real pending prompts read "Needs you".
- **Unrelated, they stay:** #483, #478, #476, #474, #473, #459, #453 and #86. (#522 was fixed on main in
  `e95a6d140`.)
- **Fixed by a step, not made obsolete:** #523 (first turn outside the worktree), by S2.

## 4. Build steps

The rules for every step:
- **Land small, land often.** Each step lands on main on its own, with its own tests. This is how this plan is
  built: the Delegator run's workers on brigadier-ai rebase, ff-merge and push. It is not a product rule;
  Brigadier's own workers land into their parent (G21), and "land small, land often" is guidance in the skill
  (S15).
- **No hidden fallback (G4).** Unfinished user-facing work stays behind an env flag. The old path is deleted in
  the step that turns the new one on.
- **Display first, runtime after.** A step that replaces what the UI shows does not delete engine code that
  other engine code still calls. That code goes in the step that replaces its **last consumer** (table below).
  This is G4's "once the new flow works".
- **Nothing is migrated.** Old records, saved settings and open sessions are not converted. A step that changes
  what a session is archives the sessions open at upgrade; there is no transition machinery.
- **The app keeps working after every step.**
- **"Done when" names real checks.** Full suites and the live smoke run once on the integrated tree, not per
  worker.
- **Who suits a step.** **Codex** = bounded and well specified, good for a Codex worker. **Opus** = needs
  judgement across the engine.
- **Parallel steps.** **∥** lists steps that touch disjoint files and can run at the same time. Steps without ∥
  run in order.

Several steps edit `M/tools.rs`, `crates/mcp-server/src/catalog.rs`, `M/prompts.rs`, `board.rs` and
`model.rs`: S1–S6, S13 and S16. They run one after another.

**Last consumers.** Engine code the display stops using, and when it may go:

| Part | Its last consumers | Deleted in |
|---|---|---|
| `M/requests.rs`, request spans in `work.rs` | turn scheduling in `M/conversation.rs`, `M/workers.rs` | S16b |
| `M/decisions.rs` | `board.rs`, `requests.rs`, `watchdog.rs`, `overnight/report.rs`, `rebirth.rs` `briefing()` | S23 |
| `M/constraints.rs` (+ the `remember` writer) | `phases.rs`, `workers.rs`, `rebirth.rs` | `remember` in S18; the module in S23 |
| `M/export.rs` | `rebirth.rs` briefing exchanges, Brain transcript sync (`brains.rs:1218`) | S23 (done) |
| `orch:<id>` log (`OrchestratorLogged`) | the resume id (`read_native_id`, `forget_native_session`), the context size (`Core::context_usage`), the thread rows (`thread_items.rs`), the Inspector's log, injections and context chart | a later step, from the CLI's own session files or `record/` (G11), never a Brigadier copy (#540) |
| `keep_note_decisions`, the rebirth Brain section | `rebirth.rs` | S23 |
| `M/context_pack.rs`, `M/worker_handoff.rs` | `M/workers.rs` | S16b |
| `M/background_runs.rs` | Brigadier's `run` and `run_check` (gone in S4, S6) | S12 |
| `machine/mod.rs` heat guard | held workers | S16b |
| `M/landing.rs` (what S5 left) | `land_phase`, overnight merges | S16c |
| `crates/mcp-server` | the old thread and worker tools | S16c |

### Stage 1: stop pushing and gating; the CLIs' own tools on (relief first)

**S1. No engine pushes (G1).** Opus. It is the first step, so it also ends what was open before.
- **Scope:** delete everything in §3.1. A thread's turn now starts only for:
  - a user message;
  - a result of work it started itself (worker reports and session events, until S16 turns those into pulls);
  - later, the seam nudge.
- **Files:** `M/status.rs`, `M/project_sessions.rs:486–578`, `M/prompts.rs` (revisions, notes, `[quiet]`,
  `[next:]`), `M/conversation.rs` (deliver, quiet and next filters, notes), `M/inbox_store.rs`, `M/workers.rs:1854`.
- **Done when:**
  - A new flow test drives a thread through 2 hours of simulated engine events and gets **no** turn: a status
    timer, another session merging, a settings change and a rule revision.
  - A user message still starts a turn, and a worker report still does.
  - `git grep -n '\[status due\]\|\[quiet\]\|\[next:\|From now on'` finds nothing in non-test code.
  - Sessions open at upgrade are archived, not carried over: a flow test starts the daemon on a data folder with
    an open session and finds it archived. There is no transition for their old instructions.
  - `cargo test -p brigadier-core` passes for the touched modules.

**S2. Long commands run as plain shell commands (G6 groundwork).** Opus. It lands before S4 removes `run_checks`
and `run`.
- **Scope:**
  - Stop setting `CLAUDE_CODE_DISABLE_BACKGROUND_TASKS=1`, `BASH_DEFAULT_TIMEOUT_MS`/`BASH_MAX_TIMEOUT_MS`
    (120000 ms) and `BASH_MAX_OUTPUT_LENGTH` (150000) for threads (`M/thread.rs:44–48`, :388–391). Claude Code's
    own defaults and background tasks hold.
  - Remove any Brigadier ceiling on Codex's command timeouts in `P/codex/mod.rs`.
  - The thread runs in its worktree (or the Local checkout), never in `<data>/orch/<id>` (#523), so plain
    commands and the CLIs' own instruction discovery see the repo.
- **Files:** `M/thread.rs`, `M/conversation.rs` (CLI cwd), `P/codex/mod.rs`, `P/claude/mod.rs`.
- **Done when:**
  - A provider test asserts none of these env vars is set for a thread.
  - A live dev session on each provider runs `tools/full-checks.sh` through plain Bash, in the background, for
    over 20 minutes, and the result reaches the thread when it ends.
  - A flow test: a new session's first turn runs in its worktree.
- **Closes:** #523.

**S3. The CLIs' own subagents and task control, on both providers (G12.read).** Opus.
- **Scope:**
  - Claude: the thread's built-in tools are Claude Code's own. Lift the exclusions of `Agent`/`Task`, `Monitor`
    and `TaskStop` (`THREAD_TOOLS`, `P/claude/mod.rs:94`). Brigadier adds no model rule of its own (G18).
    What the user's rules file (S13b) forbids reaches Claude Code's own `availableModels`, so subagents follow
    the same file; when the file forbids nothing, nothing is set.
  - Codex: drop `multi_agent`, `multi_agent_v2`, `goals` and `sleep_tool` from `THREAD_FEATURES`
    (`P/codex/mod.rs:131`) and `agents.enabled=false` (:144) for threads. Codex can't hold a subagent to a model
    list; its subagents run Codex models only.
  - Amended by run2-D43 (10-24): a session's and a Chat's thread starts with its CLI's subagent tool off
    again (Claude `--disallowedTools Agent,Task,Workflow`: its Workflow tool runs sub-agents too; Codex `-c agents.enabled=false`); workers keep theirs.
    `Monitor`, `TaskStop`, goals and the sleep tool stay on.
- **Files:** `P/claude/mod.rs`, `P/codex/mod.rs`, their provider tests.
- **Done when:**
  - Provider tests assert the thread's tool and feature flags on both CLIs.
  - A live check on each provider: the thread starts a subagent that reads files and returns conclusions, and
    starts a background task and stops it (Claude `TaskStop`, Codex `/stop` or its app-server equivalent).

**S4. Checks are plain commands; no lease, no sandbox gate (G6).** Opus.
- **Scope:** §3.2.
  - Remove the checks tools and the check gate in `merge_rule`. A merge no longer waits for a Brigadier verdict.
  - Remove the build lease, the `Nested` and skip test machinery, and the `checks-sandbox` CI job.
  - Thread prompt: "run `tools/full-checks.sh` yourself (in the background for long runs) and read its output".
    This relies on S2: long plain commands already work.
- **Files:** `M/integrated_checks.rs`, `M/checks.rs`, `M/test_counts.rs`, `crates/core/src/machine/builds.rs`,
  `machine/heavy.rs`, `M/machine.rs`, the lease calls in `M/preview.rs`, `M/flow/mod.rs:1233–1302`, the skip
  sites in §3.2, `crates/daemon/src/terminals.rs`, `.github/workflows/ci.yml`, `M/landing.rs` (gate only),
  `M/tools.rs`, `catalog.rs`.
- **Done when:**
  - `catalog.rs` lists no `run_check(s)`.
  - A thread that runs `tools/full-checks.sh` through its own Bash sees the real exit code and output in a tool
    row (flow test with the fake CLI).
  - Two sessions' heavy commands start at once, without a lease row.
  - `ci.yml` has no `checks-sandbox` job.
  - The tests of the touched modules pass.
- **Closes:** #514, #515, #479.

**S5. Reviews and merges are plain commands (G6).** Opus.
- **Scope:** §3.3. Remove the review tools, the findings ledger, the merge-anyway path and `MergeRefused`.
  - The thread runs `codex review …` and `git merge` / `git push` itself.
  - Worker landing (`land_phase`) keeps working, without any gate, until S16. After S16 a worker merges into
    its parent itself (G21).
- **Files:** `M/review_runs.rs`, `crates/review`, `M/fusion.rs`, `M/unreviewed.rs`, `M/merge_consent.rs`,
  `M/review_hold.rs`, `M/landing.rs` (consent and refusal), `board.rs`, `work.rs`, `model.rs`, `sessions.rs`,
  `ipc/src/protocol.rs`, `crates/daemon/src/server.rs`, `D/app/conversation/Activities.tsx`, `backgroundRows.ts`,
  `prompts.rs`, `tools.rs`, `catalog.rs`.
- **Done when:**
  - No "merge anyway" text is left in `crates/` or `apps/` (`git grep -i 'merge anyway'`, tests aside).
  - A flow test: the thread asks to merge in words, runs `git merge` itself, and the sidebar reads Merged from
    git.
  - `pnpm typecheck` and the app tests for the touched files pass.
- **Closes:** #519, #518 (card half).

**S6. Permissions pass straight through; no Brigadier sandbox (G5).** Opus.
- **Scope:** the composer picker becomes **Ask / Accept edits / Full access**:

  | Picker | Claude Code | Codex |
  |---|---|---|
  | Ask | `default` | `untrusted` + `workspace-write` |
  | Accept edits | `acceptEdits` | `on-request` + `workspace-write` |
  | Full access | `bypassPermissions` | `never` + `danger-full-access` |

  - Codex's own sandbox counts as the CLI's own setting and stays.
  - Nothing is migrated: saved levels are dropped, and the picker starts at Accept edits.
  - The CLI's own permission prompts keep showing as cards: `--permission-prompt-tool stdio` → `open_cli_card`
    at `M/cards.rs:144`, and Codex approval requests.
  - Delete §3.4.
- **Files:** `P/cli.rs`, `crates/sandbox/src/{macos,linux,windows}.rs` (sandbox parts only), `M/protection.rs`,
  `M/escalation.rs`, `M/run.rs`, `M/preview.rs` (its confine), `M/workers.rs` (`access_for`, `sandboxed`),
  `M/thread.rs` (`thread_access`), `P/claude/mod.rs` (`permission_mode` :637, settings), `P/codex/mod.rs`,
  `D/app/shell/SetupPickers.tsx`, `SandboxLimitNotice.tsx`, the IPC setup types.
- **Done when:**
  - Provider unit tests assert the exact flags for each of the three levels on both CLIs.
  - A flow test: under Ask, a CLI permission prompt becomes a card, and answering it continues the turn.
  - `git grep -n 'confine\|protect(' crates/core crates/providers` shows only `crates/sandbox` paths still used
    by previews' process groups, or nothing.
  - On a dev build, a session under Accept edits opens a pty: `tools/full-checks.sh` runs in it.
- **Closes:** #521.

### Stage 2: the UI reads the CLI and git

**S7. One folder per session (G13).** Opus.
- **Scope:** new sessions get `~/.brigadier/s/<short-id>/{record,work,tmp,dev,logs}` (short id: 8 characters,
  no spaces).
  - The worktree goes under `work/`, previews and dev servers under `dev/`, logs under `logs/`.
  - `TMPDIR` and `TMPPREFIX` point at `tmp/`. This replaces `session_temp_dir` at `M/workers.rs:5832`.
  - `CLAUDE_CODE_TMPDIR` stays a short `/tmp/brigadier-<12hex>` (§8, item 3).
  - Sessions made before this step are archived at upgrade, not converted.
  - Storage shows size per session folder.
  - The environment and base branch chosen on the rail (Local checkout, or New worktree + its base branch) go
    into `record/workspace.json` (environment, branch, base). A New worktree's branch also records its base
    in git config, `branch.<branch>.gh-merge-base`, the key T3 Code records and `gh pr create` reads. Later diffs,
    landing, `brigadier show` and cleanup read the target from there and never guess it.
- **Files:** `crates/sandbox/src/paths.rs`, `M/mod.rs` (`owned_dir` :889), `M/workers.rs:2532–2700` and
  `:5812–5866`, `M/thread.rs:366–395`, `M/preview.rs:78/529`, `M/disk/mod.rs`, `D/` Storage page.
- **Done when:**
  - A flow test: a new session's CLI sees `TMPDIR` under its folder, and its worktree under `work/`.
  - A Unix socket made under `$TMPDIR` by `python3 -c 'import socket…bind'` binds on macOS.
  - Storage lists the folder with its size.
  - A flow test: a New worktree session's `record/workspace.json` and `git config branch.<branch>.gh-merge-base`
    both name the base chosen on the rail; a Local checkout session's record names its branch.
- ∥ S11.

**S8. The CLI's files are the record.** Codex.
- **Scope:**
  - Archive and Delete stop removing the CLI transcript files of a live lineage. Delete still removes them
    (G15, S20).
  - Every CLI session start appends `{provider, session_id, file, started_at}` to `record/lineage.json`.
  - The ledger's `ClaudeSession`, `ClaudeProjectDir` and `CodexThread` artifacts move to "kept until Delete".
- **Files:** `M/lifecycle.rs:1040–1075`, `crates/core/src/ledger.rs`, `P/model.rs:1486`, the CLI start path in
  `M/conversation.rs`.
- **Done when:** a flow test archives a session and finds its CLI session file and `lineage.json` still there.
  Restore resumes it with `--resume`.
- ∥ S11.

**S9. Thread reader from the CLI's session file, behind a flag (G11).** Opus.
- **Scope:** a reader that turns a lineage's files into thread items:
  - Claude JSONL, including `tool-results/` and subagent sidechains;
  - Codex rollout.
  - While the CLI runs, the live stream extends the items. Stream items and file items are matched by the
    CLIs' own message and tool-call ids, not by file positions (streamed items carry no byte offsets).
  - Turn = user message → the CLI's turn-end event. "Worked for Xm" comes from the CLI timestamps.
  - The session files hold no `result` records (checked on real Brigadier JSONLs, 10-22). So Brigadier writes
    the minimum the file lacks, from the live stream, to `record/turns.jsonl`: per turn, the user message id, the
    end state (done, stopped, failed, interrupted), the error text if any, and the end time. Nothing else of
    the conversation is copied.
  - It is shown when `BRIGADIER_CLI_THREAD=1`.
- **Files:** a new `P/claude/session_file.rs`, `P/codex/rollout.rs` and `crates/core/src/thread_view.rs`, plus
  IPC events. `D/` thread renderer reads the new items under the flag.
- **Done when:**
  - Fixture tests: real JSONL and rollout files from this Mac, scrubbed, give the same items in the same order
    as the CLI shows. That covers tool rows, subagents, images, a failed turn and a stopped turn.
  - Restart parity: a flow test with the fake CLI ends turns as done, stopped, failed and interrupted (the CLI
    killed mid-turn), restarts the daemon, and the reloaded thread shows the same items and end states as before
    the restart.
  - Under the flag, a live dev session renders the same conversation as the CLI's own `--resume` view. This is
    checked by hand on two sessions.
  - Benchmarks are not part of this check.

**S10. Flip the thread; work block and sidebar from CLI events.** Opus. Display only.
- **Scope:** the flag goes away and the new reader is the only path.
  - Sidebar words:
    - **Working** = a turn is open.
    - **Needs you** = an open CLI permission prompt, ask-user card or MCP sign-in.
    - **Ready, Merged, Reverted** = from git.
  - Delete what only the display used: `thread_items.rs`, the UI's reads of `orch:<id>`, the request spans in
    the UI, and the engine half of `tabStates`.
  - Keep, for their last consumers (§4 table): `requests.rs` (S16b), the `orch:<id>` log and `export.rs` (S23).
- **Files:** as in §3.7, plus `D/state/tabStates.ts`, `activity.ts`, `requestTime.ts`, `WorkHeader.tsx`,
  `WorkFold.tsx`, `RequestBlock.tsx`, `blocks.ts`.
- **Done when:**
  - A shared test table (successor of `session_states.json`) covers each sidebar word from CLI events and git
    only.
  - The app tests pass.
  - A dev session shows the same items as `claude --resume`.
  - No UI code reads `OrchestratorLogged` or request records.

**S11. Changes card from git snapshots (G8).** Codex.
- **Scope:**
  - At each turn's start and end, snapshot the worktree as a git tree object. Use a temporary index with
    `git add -A` and `write-tree`, including untracked and excluding ignored, and keep a ref under
    `refs/brigadier/s/<id>/turn-<n>-{start,end}`.
  - The refs are the session's own: the cleanup ledger records the namespace. Delete removes all of it (S20).
    Archive keeps the refs its Changes cards show.
  - The card is the exact diff between the two: files, +/− and the diff. Worker cards show each worker's own
    +/− against the parent's branch (G21).
  - Files a merge brought in are not the turn's own change. When the turn's range (start ref to end ref, on
    the branch's history) holds merge commits, the files those merges brought in are left out of the parent's
    card and grouped by the merged branch. When that branch is one of the session's workers (S13), they show
    under that worker's card. Prior art: T3 Code's turn diff leaves out what a pull, merge or rebase brought
    in (its `docs/user/source-control.md`, lines 6–11).
  - Revert restores the start snapshot, after a confirm. Files changed after the turn are named first.
  - Delete `request_changes.rs`, `pending_changes.rs`, `shell_changes.rs`, the capture hooks and `undo.rs`'s
    recorded-commit revert.
  - The card keeps its look (bug fix ≠ redesign).
- **Files:** a new `crates/git/src/snapshot.rs`, `M/` turn hooks in the CLI pump, `D/app/conversation/ChangesCard.tsx`,
  `ChangesFileRow.tsx`, `RevertConfirmRow.tsx`, `state/pendingChanges.ts`.
- **Done when:**
  - Unit tests on a temp repo: new, deleted, renamed, binary and untracked files, and a turn with no change.
  - A flow test: Revert restores the start tree exactly (`git diff` against the start ref is empty).
  - A flow test: Delete leaves no `refs/brigadier/s/<id>/` ref; Archive keeps them.
  - A flow test: a worker card's +/− is its diff against the parent's branch.
  - A flow test: a turn that merges a branch and edits one file itself shows only that file on the parent's
    card, and the merged files under the merged branch.
  - App tests pass.
- **Closes:** #256.
- ∥ S7, S8, S9.

**S12. Background rows = the CLI's own (G10).** Codex. The CLIs' background tasks are already on (S2).
- **Scope:**
  - **Claude rows:** from stream-json `task_started`, `task_progress`, `task_notification` and
    `background_tasks_changed`. These are already parsed at `P/claude/parse.rs:884–903`.
    - Label = the CLI's description. Elapsed time from its timestamps. Latest output line from `output_file`.
    - Actions: open the output or transcript; stop.
    - Drop rules as Claude Code's own: a done subagent stays 30 s; failed and stopped tasks leave; shell rows
      clear about 5 min after exit.
  - **Codex rows:** its background terminals, as `/ps` lists them.
  - Delete `M/background_runs.rs`, the Brigadier-owned run rows in `Activities.tsx` and `backgroundRows.ts`, and
    `run`'s background mode.
- **Files:** `P/claude/parse.rs`, `P/codex/parse.rs`, `M/background_runs.rs`,
  `D/app/conversation/Activities.tsx`, `backgroundRows.ts`, `activity/BackgroundRunRow.tsx`.
- **Done when:**
  - Fixture test: a stream with a `run_in_background` Bash and a background subagent gives two rows that appear
    and drop on the documented timings.
  - A live dev session: `sleep 30 &` through Ctrl+B-style `run_in_background` shows a row and clears.
  - Stop ends the task.
- ∥ S11. It comes after S10 (shared item model).

### Stage 3: teams = the Delegator, built in

**S13. The `brigadier` CLI, with worker commands equal to `dlg` (G3, G6, G16, G19, G23).** Opus.
- **Scope:** one command, `brigadier`, built like `cmux`.
  - It is one binary, shipped in the app and put on each session's `PATH`.
  - Output is text by default and `--json` on every subcommand.
  - It reaches the daemon over its socket. The calling session is known from env (`BRIGADIER_SESSION` and a
    per-session token), checked like the MCP grants today.
  - Its arguments and output never name app-internal paths, and subcommand names are stable, so it can ship as
    a product CLI later.
  - Subcommands:

  | Command | What it does |
  |---|---|
  | `brigadier sessions` | List the project's sidebar sessions: title, state from CLI events, branch, +/−. Workers are not in it. This is "read anything"; `brigadier show <id>` gives a session or worker in full. `--all` lists every project's sessions (S30) (user, 2026-10-23, w95 item 12: every project). |
  | `brigadier workers` | List the calling session's own workers: title, agent, model, state from the worker's own CLI events, branch, +/−. |
  | `brigadier start` | Takes `--title`, `--agent claude\|codex`, `--model`, `--effort`, `--worktree\|--local` (default `--worktree`), `--base` (default: the parent's current branch, G21; recorded in the worker's `record/workspace.json` and `gh-merge-base` like S7), `--permission` (default: the parent's level, Q8), `--lean` with `--skill <name>` (G23, below) and the prompt. It starts a worker of the calling session, in that session's project (G19). The worker shows only inside that session (its Workers row, rail and worker cards), never in the sidebar. It refuses only what the user's rules file forbids (S13b), quoting the rule, and a call from a worker: a worker can't start workers, and uses its CLI's own subagents instead (user, 2026-10-23: "workers cant start other workers"). |
  | `brigadier send <id> <text>\|--allow\|--deny` | To one of the caller's own workers, queued mid-turn, like `dlg send`. This is how the parent relays the user's answers (G19.Q6). `--allow` or `--deny` answers the worker's open CLI permission prompt through the CLI's own permission protocol, never key presses (Q8). The prompt reaches the parent as a `brigadier wait` "needs input" event, and until it is answered it shows under Waiting on you in the parent. |
  | `brigadier wait [<id>…] [--timeout]` | Blocks until an event from one of the caller's workers: turn ended (with the last message), needs input, gone, usage limit, fresh context. Long waits run in the CLI's background, which re-invokes the parent when one ends (S29) (user, 2026-10-23: "The parent should know everything"). |
  | `brigadier read <id> [--last N\|--screen]` | The last message, the last N items or the screen, of any session or worker. |
  | `brigadier close <id>` | Closes one of the caller's own workers. |
  | `brigadier queue <id> join\|wait\|worker <worker-id>\|new-session` | The calling session's judgment of a message the user sent while it worked, shown in the rail as Deciding (run2-D40; "Deciding" above). The rail updates from the queue state this writes. |
  | `brigadier preview start\|stop\|log`, `brigadier browser …`, `brigadier ask …`, `brigadier app …`, `brigadier page …` | The tools the thread keeps today (previews, browser, ask-user card, app actions, pages), moved as they are. `brigadier ask` blocks until the card is answered. Its card always allows a free-text answer next to any options, never options only; the recommended option comes first, so "yes" takes it. |
  | `brigadier models` | Live models, usage and guidance (S13b). |

  - Brain search and the code index get their subcommands in S18.
  - **`brigadier start --lean` (G23)** starts a research worker with just the goal and the repo:
    - no user MCP servers;
    - no skills beyond those named with `--skill <name>` (none by default);
    - no memory writes (Claude `autoMemoryEnabled:false`, Codex `features.memories=false`);
    - the user's instruction files still load (G17 keeps project instructions).
    - It is built from the CLIs' own flags and settings: `--strict-mcp-config` with an empty MCP config and a
      settings JSON for Claude, config overrides for Codex. The user's files are never edited.
  - `workers` is its own command, not `sessions --mine`: the two list different kinds of thing (sidebar
    sessions vs. the caller's workers), so each keeps one fixed `--json` row shape, and `workers` lists exactly
    the ids that `send`, `wait` and `close` accept.
  - `send`, `wait` and `close` act only on the caller's own workers; another session's worker is refused.
    Sidebar sessions message each other through `brigadier send` and `wait` on a session id, when the user
    asks or when it is absolutely needed (S30) (user, 2026-10-23: "Sessions should be able to message eachother").
  - `peers.rs`, `peer_wait.rs` and `peer_records.rs` go. Their parent → own worker parts move into the worker
    commands; session-to-session messaging comes back in S30 (user, 2026-10-23: "Sessions should be able to message
    eachother"). `message_session`, `wait_for_replies`, `settle_wait` and `offer_session` go. The kept tools leave
    the MCP catalog in the same step; the MCP server keeps only the old protocol tools until S16.
  - Computer use stays the vendored driver's own interface (`M/computer_use.rs`), not part of the `brigadier`
    subcommands, and it reaches every session, Chat and worker while its setting is on (S35) (user,
    2026-10-23: "fix this!").
  - There is no land or merge subcommand. A worker merges into its parent with plain git, and the parent
    merges on into the branch the user chose; Brigadier enforces nothing there (G21).
  - Brigadier enforces no model rule of its own (G18). The model refusal is `brigadier start`'s check against the
    user's rules file, built in S13b; the other is a worker starting workers (above) (user, 2026-10-23: "workers
    cant start other workers").
  - Non-goal: an MCP wrapper over the same commands. It would come only if a client can't run shell.
- **Files:** a new `crates/cli` (the `brigadier` binary), a daemon request handler for it, `M/peers.rs`,
  `M/peer_wait.rs`, `crates/core/src/sessions/peer_records.rs`, `catalog.rs`, `tools.rs`, the app bundle and
  `PATH` setup in `P/claude/mod.rs` and `P/codex/mod.rs`, the parent → worker link in the session store.
- **Done when:**
  - CLI tests: every subcommand's `--json` output matches a checked-in schema, and `brigadier --help` lists them.
  - Flow tests with the fake CLI: start two workers, send one a message mid-turn, and `brigadier wait` returns
    both turn ends with their last messages. Close removes them.
  - A flow test: the workers are listed by `brigadier workers` and not by `brigadier sessions`, and the sidebar
    has no row for them.
  - A flow test: `brigadier start` with no `--base` and no `--worktree\|--local` makes a new worktree whose
    branch starts at the parent's current branch, and the worker's record and `gh-merge-base` name that branch.
  - A flow test: a worker started with no `--permission` runs at the parent's level; its permission prompt
    reaches the parent as a "needs input" event, shows under Waiting on you, and `brigadier send <id> --allow`
    continues the worker's turn.
  - A call without a valid session token is refused, and so is `send` or `close` on another session's worker.
  - A live dev check on each provider: the thread starts a Codex worker through `brigadier start` that writes
    a file and reports.
  - An app test: the ask card takes a free-text answer when it also shows options.
  - A provider test per CLI asserts the exact flags and settings `--lean` (with and without `--skill`) passes.
  - A live lean worker lists its tools and shows none of the user's MCP servers.

**S13b. `brigadier models`: routing as advice (G18).** Codex.
- **Scope:** `brigadier models [--json]` merges three layers into one view:
  - **Live state:** the models each CLI reports as available, usage and reset times per account, and the
    current account. It reads what `crates/core/src/routing/*` and the accounts code already hold.
  - **Shipped guidance:** a plain, editable file shipped in the app like the `/brigade` skill
    (`crates/core/src/manager/models_guidance.md`): which provider, model and effort fits what kind of work,
    with a one-line reason each. It is updated per release.
  - **The user's rules:** the user's own file (`~/.brigadier/models.md`), layered on top. Its rows override the
    shipped rows; its rules (forbidden models, an effort ceiling, allowlists) are shown as such. The shipped
    default of this file is neutral and forbids nothing; the user's own rules (never Fable, effort ≤ high for
    this user) live in the user's own file, which the user may edit or empty (S33) (user, 2026-10-23, w95
    item 16). Settings › Routing edits the same file (S32).
  - `brigadier start` refuses a model or effort only when the user's file forbids it, and quotes the rule.
    Brigadier enforces nothing else; there is no built-in list.
  - The `/brigade` skill tells the thread to consult `brigadier models` before `brigadier start` (S15).
  - Account auto-switch stays; S38 orders the accounts smartly and adds the order and both toggles to
    `brigadier models` (D23).
- **Files:** `crates/cli` (the subcommand), the daemon handler, the guidance file, `crates/core/src/routing/*`
  (read only), the account usage reader.
- **Done when:**
  - The `--json` output matches a checked-in schema.
  - Unit tests: a user row overrides the shipped row; a user rule shows in the merged view.
  - A flow test: with a rules file that forbids Fable, `brigadier start` with a Fable model is refused and quotes
    the rule; after the user removes that rule, the same call starts the session (S33 makes the shipped default
    neutral; user, 2026-10-23, w95 item 16).
  - `git grep` finds no other model refusal outside the rules-file check.
  - A live dev check: the listed accounts' usage and reset times match Settings › Usage.

**S14. Context card per G9/G10.** Opus.
- **Scope:** the card from §1:
  - the branch row (git);
  - the PR row: `#N` + state word + colour from `gh pr view`, refreshed when the CLI's events show a
    `git push` or `gh pr` command; failed checks red. The words, colours, light poll and sign-in hint are §1's
    list, built in S34 (user, 2026-10-23: "fix this");
  - Waiting on you (real pending prompts and questions of this session's workers, G19.Q6);
  - Workers (this session's own workers from `brigadier start`, S13: title, state from the worker's own CLI
    events, +/− of its worktree against the parent's branch (G21), "N done · M stopped");
  - Background (S12);
  - Sources: attachments first, then URLs from the CLI's fetch and browser tool items, then MCP servers seen in
    its events; + adds one.
  - A "Resumes at" card from the CLI's usage-limit error.
  - Nothing reads `Board` engine records any more.
  - **The worker UI keeps its look (G19)** and gets the same feed as the Workers row: the rail above the
    composer, the worker cards and the worker panel. Workers never show in the sidebar.
  - **Read-only workers (G19.Q6):** opening a worker from the rail shows its thread (the S9 reader on the
    worker's own files), with no composer. The user answers a worker's prompt or question in the parent, and the
    parent relays it with `brigadier send`. An `@worker` mention in the parent's composer is a routing hint for
    the parent only; it sends nothing to the worker.
- **Files:** `D/app/conversation/SummaryContent.tsx`, `PinnedSummary.tsx`, `FloatingSummary.tsx`,
  `side-panel/WorkerSummary.tsx`, `WorkerList.tsx`, `WorkerThread.tsx`, `activity/WorkersStrip.tsx`,
  `stripView.ts`, `SourcesTab.tsx`, `M/pull_request.rs` (read only; merge queue and CI watch go), `M/mod.rs:663`
  (`session_diff_stat`).
- **Done when:**
  - App tests per row with fixture events.
  - A flow test: after a `git push` item, the PR row refreshes.
  - A dev session with a started worker shows the worker's state and +/− in its Workers row and rail, and the
    worker has no sidebar row.
  - An app test: a worker opened from the rail shows its thread and no composer.
  - No component under `SummaryContent` imports `boardJob` or `liveJob`.

**S15. The `/brigade` skill, always on (G3, G19.Q5, G20, G21, G23).** Opus.
- **Scope:** one plain, editable skill, the only one Brigadier ships: `crates/core/src/manager/brigade/SKILL.md`,
  included like `computer_use_skill.md`. The public `brigadier-skill` repo is unrelated: it is for outside CLIs
  talking to the app.
  - It is adapted from `~/.claude/skills/delegator/SKILL.md`:
    - the `dlg` commands become `brigadier` subcommands (S13);
    - cmux tabs become workers of the calling session (G19);
    - the user answers workers in the parent, and the skill relays with `brigadier send` (G19.Q6);
    - "hand off at 300k" becomes the seam;
    - overnight is the same skill with a deadline;
    - workers run their own checks and merge into the parent (G21);
    - the thread consults `brigadier models` before each `brigadier start` and picks provider, model and effort
      itself (G18, S13b).
  - Integration guidance, not rules (G21). Brigadier enforces none of it:
    - Land small, land often: a worker merges each finished step into the parent, not one big merge at the end.
    - A worker branches from the parent's current branch (`brigadier start`'s default `--base`).
    - The parent decides merge or rebase and resolves conflicts, asking the worker for help when it knows the
      change better.
    - The parent integrates; the checks on the integrated tree run in a worker or CI, never in the parent
      itself (run2-D24); then it merges into what the user chose: the New worktree's base branch, or the Local
      checkout's branch chosen from the rail.
  - The orchestrator default, worker reuse, grilling and the short reply as in "Teams" above (run2-D23…D43).
  - Research and review guidance (G23, as amended by run2-D32…D43). All of it is guidance; nothing is a gate:
    - Research goes to lean workers (`brigadier start --lean`, S13) on the routing's search row; the session
      makes only quick look-ups itself (run2-D43).
    - When the user is uncertain, help them: a model fusion gives the best advice or answer, through `/fusion`
      or the thread's own judgement (S36) (user, 2026-10-23: "if the user is uncertain - we help him").
      One lean researcher when the options need evidence. Builders only when prototypes must be compared,
      never by default.
    - Adversarial reviews, like the Delegator's: a big job's first plan gets a plan review before it goes to
      workers, and the big job a code review once at its end on the integrated tree (run2-D33). The reviewer
      sets out to break it, with evidence, never to approve; it is a `--lean` worker started with `--work
      "Independent review"`, best on another vendor or model than the author; the session fixes the valid
      findings through workers and notes each rejected one with its reason (run2-D35, run2-D36). A small change
      with its tests passing lands without a review or a critique (the w123 bench, 10-23: on a two-file job
      these rules tripled a Codex thread's time). "Asking once" (G23) gives way to grilling (run2-D32).
  - A Chat starts workers like any session; a worker of a Chat runs in a folder of its own under the Chat's
    session folder unless the Chat names a repo (S28) (user, 2026-10-23: "a chat is always a brigade").
    A worker can't start workers; it uses its CLI's own subagents (user, 2026-10-23: "workers cant start
    other workers").
  - Gotchas the skill states, one line each (`research/POCOCK.md` §4):
    - Worktrees don't remove collisions; they move them to merge time. Two workers that touch one shared file
      (a registry, a type, a string catalogue) run in order, or get their exact names fixed up front.
    - Tests that need gitignored files (fixtures, local databases, credentials) can skip silently in a fresh
      worktree. The worker copies or symlinks those files in, and says which tests skipped.
    - Review runs once, at the end, on the integrated tree, never mid-run. After the fix pass only focused
      checks run, with a stop rule: one review, one fix pass, then report what is left instead of looping.
  - Every session and chat is a brigade (G20). The skill is installed for every one of them, per process:
    Claude's `--plugin-dir` and Codex's per-app-server skills root (`skills/extraRoots/set`), so a plain
    `claude` or `codex` never lists it (§8, item 7), and the standing instructions point at it (§6, S17).
    (Until 10-23 Codex got a copy in the user's `~/.codex/skills`, which every plain `codex` listed (the
    w123 bench); one pass at the next Codex session removes that copy, and a marker keeps it to one pass.)
    No one has to invoke it and there is no "New brigade" UI. It stays lazily loaded: only its description
    costs context until the session uses it.
  - `BRIGADIER_BRIGADE_SKILL=1` is only the build gate: the skill and its pointer line ship behind it until the
    done-when passes, then the flag goes and both are always on. It is never an opt-in for the user.
- **Files:** the skill file, `M/computer_use.rs`-style installer, `P/claude/mod.rs` (`--plugin-dir` / skill
  path), `P/codex/home.rs`.
- **Done when:**
  - A live run on a dev build: a plain request to a session, with no `/brigade`, leads it to a two-worker change
    (one Claude, one Codex) through the skill. Each worker branches from the parent's branch, runs its own checks
    and merges into the parent; the parent integrates and merges into the test repo's main, the base the user
    chose.
  - A provider test: every new session and chat, on both CLIs, starts with the skill in its skills folder.
  - The A/B against cmux `/delegator` on the same request is written as a report and gates nothing.

**S16. Delete the old engine.** Opus. These are three landable sub-steps, in this order, after S13–S15 are on.

- **S16a.** Phases, outlines, overnight, plan cards, the Job card and WaitCard (§3.5).
  - Done when `git grep -n 'JobCard\|set_headline\|propose_overnight\|plan_phases'` finds nothing.
  - Overnight is reachable only as the skill.
  - The app and core tests that remain pass.
- **S16b.** Engine-side worker orchestration only (§3.6): spawning, routing, holding and merging by the
  engine (workers merge into their parent themselves, G21), the worker hand-off and resume machinery, context
  packs, research, outcomes and reruns. Also their last-consumer leftovers: `requests.rs` and the request spans, and the heat guard (§4 table). The worker UI
  (rail, Workers row, worker cards and panel) stays, already fed by the worker's own CLI events since S14 (G19).
  - Done when the MCP catalog lists no worker, routing or task tool, `M/requests.rs` is gone, and `crates/router`
    keeps only the shared types.
  - A worker started with `brigadier start` still shows in its parent's rail, Workers row and worker cards.
- **S16c.** The landing engine, the CI-watch engine and the Brigadier MCP server (§3.6, §3.9).
  - Done when `M/landing.rs` and `crates/mcp-server` are gone, sessions start with no Brigadier entry in their
    MCP config (the user's own servers still load), and the PR row still works.
  - What the thread, Chats and side chats still needed over MCP moved to `brigadier` as thin wrappers, with
    unchanged behaviour: `brain search` (was `query_brain`), `transcript search`, `code search|refs|map`,
    `page read|write` and `page file-read|file-write` (side chats), `memory save` and `report save` (Chats),
    `feedback offer` and `approval request`. S18 re-points `brain search` and `transcript search`; S19 deletes
    `memory save`. `remember`, `record_nodes` and the enrichment jobs went early, as S18's last MCP users.
  - Chats and side chats had no shell, so they run `brigadier` through one limited to it: on Claude, `Bash` with
    `dontAsk` and only `Bash(brigadier:*)` allowed (Claude's own read-only commands still run in the session's
    folder); on Codex, a read-only sandboxed shell with network on for the daemon's socket, where Brigadier
    approves only a plain `brigadier` command (Codex's rules can't forbid everything else). Codex asks about
    every command, and runs an approved one outside its sandbox when the model asked for that, so Brigadier's
    approval is the guard. Codex's network proxy (`features.network_proxy`) would hold the sandbox to the
    socket, but has no effect over the app-server's per-turn sandbox policy (checked on 0.163.0).
  - For Chats this limited shell goes in S28: a Chat gets the CLI's own shell like a session, cwd = its session
    folder, and Brigadier approves nothing (user, 2026-10-23: "a chat acts almost exactly as a normal
    session"). Side chats keep the limited shell; the answers did not cover them.
- Each sub-step closes the §3.6 issues it obsoletes, and re-checks #382 and #296.

### Stage 4: long sessions (G12) and Brain (G14)

This stage can run in parallel with Stage 3 once S10 has landed. It touches `rebirth.rs`, `brains.rs`, `crates/brain`
and the seam code, not the session commands. The shared files are `prompts.rs`, `tools.rs` and the MCP catalog, so S17 and S18 rebase after S16.

**S17. Standing instructions and the ledger (Q3, G12.read).** Opus.
- **Scope:** the text in §6 becomes one file, `crates/core/src/manager/standing_instructions.md`, identical for
  every session.
  - It reaches both CLIs explicitly, never by discovery: Claude gets it with `--append-system-prompt`, Codex
    as the thread's `developer_instructions` (`P/codex/mod.rs:1151/1288`).
  - The file is one template; only the two paths it names differ per session. Brigadier writes the session's
    folder and ledger out in the text it hands each CLI, and the env vars `BRIGADIER_SESSION_DIR` and
    `BRIGADIER_LEDGER` hold the same paths for scripts. (Claude asks before any command with a variable it
    can't check, so in Accept edits a ledger named only by its variable cost the user a card per entry.)
  - One line says the session may run workers through `brigadier` and that the `/brigade` skill holds the
    guidance (G20). It is the same for every session and chat; nothing has to be invoked.
  - It replaces the thread prompt (`M/prompts.rs` `thread()` and macros) and `M/instructions.rs`. That file
    exists because the thread ran outside the repo (`instructions.rs:66–71`); after S2 it runs in the repo.
  - Repo instructions are then each CLI's own, with no parity added. Claude Code reads CLAUDE.md files as it
    always does. Codex reads AGENTS.md along the root → cwd path only: no CLAUDE.md, and no AGENTS.md in
    subfolders below the cwd. That is plain Codex behaviour, taken as is (§8, item 6).
  - Nothing of the user's is stripped (G17): project instructions, the user's MCP servers, skills and memory
    load as in a plain CLI. Brigadier's own additions are the only thing kept short.
  - The hand-off text is not in this file. It travels in the seam nudge and the successor's bootstrap (S22).
  - The ledger is not shown in the UI: Sources stay what §1 defines (attachments, fetched pages,
    searches, MCP servers).
- **Done when:**
  - A provider test asserts the exact appended text, the Codex developer instructions and the env on both CLIs.
  - A byte test keeps the file under 2,000 bytes and 15 lines.
  - A dev session writes `record/ledger.md` when told a rule.
  - A one-time measurement of what a Brigadier-run `claude -p` sends per turn, next to a plain `claude` in the
    same repo: `/context`, or our own small local proxy on `ANTHROPIC_BASE_URL` (written by us, not copied). The
    report lists each block and tool with its size. Then Brigadier denies only the tools that cannot work inside
    a Brigadier tab: `RemoteTrigger`, `PushNotification`, `ScheduleWakeup` and `Cron*` (no idle loop in `-p`),
    `AskUserQuestion` (it reaches Brigadier as a bare permission card that carries no answer back; `brigadier
    ask` asks instead), and claude.ai connectors (`disableClaudeAiConnectors`). Nothing of the user's is denied.
    A provider test asserts that deny list and nothing more.

**S18. Brain = pull-only search over the records (G14, Q5).** Codex for the reader, Opus for the deletions.
- **Scope:** re-point the transcript index (`M/brains.rs:1218–1302` `sync_transcript_stream`, `crates/brain/src/transcript.rs`)
  from the event store to:
  - every session's lineage files, including `tool-results/`;
  - every ledger, `ledger-archive.md` and hand-off note in the project.
  - Entries of kind "Learned from reading" are indexed as pointers (file:line + finding) with a content hash.
    When the file changes, they read as stale.
  - Commands: `brigadier brain search <query> [--session …]` returns hits with session, position and a text
    window. `brigadier brain read <session> <from> <to>` returns a verbatim span.
  - Delete §3.8's Brain-first rule. The `learn_report` and `record_job_nodes` writers, the `remember` tool and
    `brain_jobs.rs` enrichment already went in S16c. `constraints.rs`, `keep_note_decisions` and the event-store sync stay for
    `rebirth.rs` until S23 (§4 table).
  - The code index stays as optional commands: `brigadier code search|refs|map`.
  - These commands, and `brigadier transcript search`, already exist as thin wrappers since S16c.
- **Done when:**
  - Fixture tests: a phrase in a predecessor's JSONL, in a large tool output under `tool-results/`, and in a
    ledger is found.
  - A changed file marks its pointer stale.
  - `git grep -n 'learn_report\|enrichment\|Brain-first'` finds nothing.
  - The CLI schema test covers the new subcommands.

**S19. Delete the Personal Brain; the CLIs' own memory is on.** Opus.
- **Scope:**
  - Delete §3.8's Personal Brain code, `personal.sqlite` and the Settings memories page outright. Nothing is
    exported or migrated (user, 10-22).
  - Stop forcing `autoMemoryEnabled:false` (`P/claude/mod.rs:827`) and drop `memories` from Codex's
    `DISABLED_FEATURES` (`P/codex/mod.rs:148`). Each CLI's memory then follows the user's own setting.
  - Chats follow the same rule. A Claude Chat loads no user settings, so it is given the user's own
    `autoMemoryEnabled` as set (nothing when unset).
  - Memory stays off only where nobody's memory should grow: a lean worker (G23), a side chat (it leaves
    nothing), Brigadier's own one-shots (reviews, research, hand-off notes) and control connections.
- **Done when:**
  - Provider tests: with the user's memory setting on and off, on each CLI, the session starts with that same
    setting (Brigadier sets nothing).
  - No prompt contains "The user's preferences", and `git grep -n 'personal.sqlite\|save_memory'` finds nothing.

**S20. Archive and Delete per G15.** Codex.
- **Scope:**
  - **Archive** stops processes and removes `tmp/`, `logs/`, `dev/`, build output and a merged worktree. It keeps
    `record/`, the lineage transcripts and Brain entries, and is restorable.
  - **Delete** removes the session folder, its Brain entries, its lineage's CLI transcript files, its
    `refs/brigadier/s/<id>/` snapshot refs and the worktree. It removes the branch after the existing unmerged
    ask.
  - Archive keeps the snapshot refs its Changes cards show.
  - Archiving or deleting a session does the same to each of its workers (G19). Archive stops them first.
  - **Storage cleanup** recognises worktrees whose branch is merged into its recorded base (S7), squash merges
    included (by patch-id, or by `gh pr` state when a PR exists), that have no commits beyond the base, or that
    are idle. It suggests; it is not a gate. A dirty worktree is never removed without the existing "not
    merged" ask.
  - Update the §5 Lifecycle text in `docs/PLAN.md` to point here.
- **Files:** `M/lifecycle.rs`, `M/closing.rs`, `crates/core/src/ledger.rs`, `M/brains.rs` (`forget_transcript`
  → forget the session's entries), `M/disk/*`.
- **Done when:** flow tests list exactly what each action leaves on disk, in git refs and in the Brain, for a
  session with two lineage generations and one worker; the worker gets the same treatment as its parent.
  Unit tests on a temp repo: cleanup flags a merged, a squash-merged, an empty and an idle worktree, and a dirty
  one only with the ask.

**S21. Continuity battery v0 (Q7).** Codex. Can run in parallel with anything after S8.
- **Scope:** a local script, `tools/continuity-battery/`, with a text report. It covers the planted items and
  forced seams of `research/PROPOSAL.md` §2.6. It runs against today's rebirth, so S22 has a baseline.
- **Done when:** one run's report exists, item by item, for Claude.
- This is a report, never a gate.

**S22. Seam v1 behind `BRIGADIER_SEAM=1` (G12, Q0–Q2, Q4).** Opus.
- **Trigger:** when the hot context (from the CLI's usage events, as today) passes `BRIGADIER_SEAM_TOKENS`
  (default 200000), queue the one nudge mid-turn, the Delegator's way.
  - Wait while the CLI reports running background tasks or an open prompt.
  - One nudge per generation, recorded in `record/seam.json`.
  - The nudge carries the hand-off text of §6 (it is not in the standing instructions). It says "finish the step
    you are on" and lets the thread choose its boundary; it never says stop now.
- **Note and divider:**
  - The thread writes `record/handoff/<n>.md`.
  - Brigadier draws the divider "Fresh context · hand-off note" (openable) and closes the predecessor.
- **Successor:** starts at once and carries on from the note by itself, like `dlg spawn --continues`; there is no
  flag and it never waits for the next user message (user, 2026-10-23: "Why wait? See how delegator works"). The
  bootstrap is its first message, also saved as `record/handoff/<n>-start.md`. S22 landed with a wait by default
  and a `BRIGADIER_SEAM_CARRY_ON=1` switch; S23 makes carrying on the only behaviour and removes the switch. The
  bootstrap holds:
  - the map;
  - the whole ledger;
  - the note;
  - a verbatim tail of about 15k tokens;
  - the successor lines of §6's seam text.
  - S22b then changes this order and the seam text (`research/CONTINUITY.md`); S22 lands with the text above.
- **Edge cases:**
  - A user message during the note turn waits up to a minute, then goes to the successor, exactly once.
  - The user's Stop in the note turn closes the seam; their next message starts the successor (w116).
  - Death without a note (limit, crash, prompt too long) → the successor starts from the ledger and tail with
    the "check git first" line.
  - Daemon restart → `seam.json` decides; never two successors.
- **Probes on the side** (run 10-23 on codex-cli 0.163 and Claude Code 2.1.309):
  - Codex's opt-in 1M window, a 1-hour probe (Q0). **Held.** With `-c model_context_window=1000000 -c
    model_auto_compact_token_limit=900000` on `gpt-6.1-sol`, the server took requests of 503k, 742k and
    799k input tokens with no error. Codex caps the setting at the model's `max_context_window` of 872k (828k
    usable at its 95%) and compacted once, before a turn, past about 90% of 872k, not at 900k. The 272k
    window is the other models'; this one's default is 500k. Only this model was probed, without tool calls.
  - Whether a failure hook sees the full failing output. **No.** `PostToolUseFailure` gets the same
    ≈10 kB head-and-tail excerpt the model sees (cut from the first 30k characters), in its `error`
    field, and nothing on disk keeps the whole failing output. Only a successful run's large output is
    saved whole (`tool-results/`, `tool_response.persistedOutputPath`). Keeping a failing output whole
    needs the command itself to keep it (as the Bash hook's trimmed-output blob does).
- **Done when:**
  - Flow tests with a 60k debug threshold give two seams, the divider and note, exactly-once delivery, a death
    without a note, and a restart between note and successor.
  - The battery is run against it, and its report goes into the run's notes.

**S22b. Decisions survive the seam (`research/CONTINUITY.md`).** Opus. After S22; it may land with S23.
The user asked for this fix (user, 2026-10-23: "find a fix for this"). Why: on 10-22/23, 33 decisions and
instructions were lost or changed between the user and the work. Most were not forgotten at a hand-off. They lost
who decided them, lost their defining sentence in a summary, or waited with no moment to raise them
(CONTINUITY §1).
- **Scope:**
  - `brigadier ledger add` writes `L<n> · <MM-DD HH:MM> · gen <g> ·` before the text. Ids count up per ledger
    and are never reused. The model never types the time.
  - `brigadier ledger close <id> "<how>"` appends a closing entry. Nothing is rewritten.
  - `brigadier ledger show --open` lists open items that aren't closed; `--rules` lists rules and decisions.
    With no flag it shows everything, as today.
  - `brigadier ledger check <file>` lists the rule and decision ids the file never cites, the ids it cites that
    are closed or superseded, and the ledger's entries with no id. Text out, `--json` too. It judges nothing
    and refuses nothing; the thread reads the list and decides.
  - **Bootstrap order** (replaces S22's list): the map; open items, rules and decisions, verbatim and whole,
    never cut for budget; the rest of the ledger, newest first, within the budget, with the read command for
    what it left out; the note; **every user message of the lineage, verbatim**, within about 20k tokens / 80 KB (D49, amended 10-24; the
    first and the latest always; the read command for the rest); then L2's latest exchange and tail. The user's
    own messages catch a rule the thread never wrote down: a 2026 compaction study kept over 90% of user
    constraints by reading only the user's turns (CONTINUITY §2).
  - The §6 standing text and seam text as amended on 10-23.
  - **The `/brigade` skill** gains four lines:
    - a worker's brief carries the rules that bear on its job, verbatim, with their ids;
    - a plan or brief built from decisions cites their ids, lines of the writer's own say "proposed", and a gap
      goes to `brigadier ask`, never a silent fill;
    - before handing a derived document on, run `brigadier ledger check` on it, and give its reviewer the
      ledger with the question "what is missing, changed or added?";
    - when the user writes after a gap, raise the oldest open question first and say how many more wait.
  - **The battery's decision-survival block** (`tools/continuity-battery/`, CONTINUITY §3.7): ten planted items
    (rules, one later corrected, a principle with its defining sentence, decisions, one "I decided", open
    items, a false worker claim), three forced seams, then six checks: applied without being asked, the
    principle on a new case, a derived brief, open items raised, own decision told, recall when asked.
- **Files:** `M/cli_ledger.rs`, `crates/cli/src/args.rs` and the ledger schema, S22's bootstrap module,
  `M/standing_instructions.md`, the `/brigade` skill file, `tools/continuity-battery/`.
- **Done when:**
  - Unit tests: ids count up and survive a move to `ledger-archive.md`; `close` appends; `show --open` leaves
    out closed items; `check` on a fixture plan lists exactly its uncited, closed and no-id entries.
  - A bootstrap test: with a ledger over the budget, every open item, rule and decision is in the bootstrap
    whole, and the cut part has a read command that works.
  - S17's byte test still passes (≤ 2,000 bytes, ≤ 15 lines); the provider test asserts the new text.
  - One battery run with the decision-survival block (Claude, 60k debug seam) exists as the baseline. It is a
    report, never a gate.
- **As landed (w112, 2026-10-23):**
  - `ledger add` and `close` print the new entry's id. `close` refuses an id the ledger doesn't have; it
    rewrites nothing either way.
  - An entry's kind is read from its first words: "Rule", "Decision" (or "user decided", "I decided"),
    "Open", "Closed L<n>". "supersedes L<n>" ends the entry it names. `--open` and `--rules` read
    `ledger-archive.md` too and leave out what was closed or superseded.
  - `ledger check` also lists ids the file cites that the ledger doesn't have: a mistyped id reads as
    cited otherwise. It reads the file on the caller's side; `--json` carries the four lists.
  - The rest of the ledger gets about 8k tokens; what it leaves out is the ledger's oldest part, and its read
    command is `sed -n '1,<line>p' <ledger>`. The user's messages include those sent mid-turn; a successor's
    bare "carry on" is not one of them. Those left out for length are named by position.
  - The battery block is `tools/continuity-battery/decisions.py` (`run.sh decisions`). T1 is scored from
    the file and by running its functions; T3's judge is `claude -p --model sonnet` on a fixed rubric.
  - Baseline (Claude `opus`, 60k debug seam, two seams by size and one death with no note;
    `tools/continuity-battery/reports/2026-10-23-claude-decisions.md`): 9 of 9 applied or raised unprompted
    where they apply, 10 of 10 recalled when asked, every hand-off carried the 2 open items and 8 rules and
    decisions whole, and each successor wrote its "Took over" entry. The brief missed its bar: it cited all seven
    decisions with ids and contradicted none, but the judge counted 9 lines of the thread's own not marked
    "proposed" (some are the brief's framing, so the count is high). Two later choices of the thread's own (a
    `RangeError`, the README section) were ledgered "I decided" but not told to the user, because the request
    asked for a one-word reply.

**S23. Seam on; delete rebirth (Q6).** Opus. It runs in the same run as S22, once the battery shows the seam no
worse than today's rebirth.
- **Scope:** the flag defaults on. The successor carries on at once from the note: delete the wait for the next
  user message and the `BRIGADIER_SEAM_CARRY_ON` switch (user, 2026-10-23: "Why wait? See how delegator works").
  Delete §3.8's rebirth, briefing, cold, overflow, threshold and stand-in reseed code. With `rebirth.rs` gone, its
  last-consumer leftovers go in the same step (§4 table): `decisions.rs`, `constraints.rs`, `keep_note_decisions`,
  the `orch:<id>` log, `export.rs` and the Brain's event-store sync.
  - A usage limit shows the "Resumes at" card, then the session continues visibly at the reset — a divider +
    the same turn resumed (MA5) — like the plain CLI (Q11, §9).
  - An account switch or a model change continues the same CLI session; it never starts a successor
    (user, 2026-10-23: "sure, continue the same session, don't start a fresh one").
  - Sessions open at upgrade are archived.
- **Done when:** `git grep -n 'rebirth\|briefing\|BRIGADIER_REBIRTH_TOKENS\|OrchestratorLogged'` finds only
  docs, and the S22 tests pass with no flag.
  - A flow test: after the hand-off note, the successor's first turn starts with no user message, and
    `git grep -n BRIGADIER_SEAM_CARRY_ON` finds nothing (user, 2026-10-23: "Why wait? See how delegator works").
- If S22b has landed, the seam-versus-rebirth comparison includes its decision-survival block, with the same
  planted items on both arms.
- **As landed (w108, 2026-10-23):**
  - The `orch:<id>` log stays, so the done-when grep still finds `OrchestratorLogged` in code. Its last
    consumers were not only rebirth: the resume id, the context size, the thread rows and the Inspector still
    read it (§4 table). A later step takes all four from the CLI's own session files or `record/` (G11), never
    a Brigadier copy, and deletes the log (#540). The variant was not renamed to pass the grep.
  - The stand-in is deleted whole (`Conversation.fallback`, `ModelFallback`, its event, pill and usage row): a
    usage limit with no other account waits for the reset (MA5). Old stand-in events read as retired.
  - MA5: the wait after a reset is spread at random over a minute; at most 4 resumes in a row go by
    themselves, then the wait says "goes when you choose Continue now"; a turn the CLI had begun goes on with a
    carry-on note, never the message again. All of it survives a restart.
  - A generation that dies of "prompt is too long" keeps its request working: the successor that carries on
    serves it (rebirth did the same).
  - `decisions.rs` is deleted; old decisions show their stored words.
  - L5 has no weekly scan to go into yet: it moves to S25's weekly sample.
  - The continuity battery and `tools/ab/v2` are seam-only; the rebirth-only A/B scripts are deleted.

**S24. Cold-cache rule.** Dropped (Delegator's judgement, 2026-10-23: the research's saving is one uncached
read per cold return; plain Claude Code never restarts on a cold cache; S24 stays dropped; the nightly
battery reports cold-return cost, never gates).
A session the user returns to after the cache TTL resumes as the plain CLI does. There is no idle nudge (Q2).

**S25. Battery nightly (Q7).** Codex.
- **Scope:**
  - The battery runs nightly on both providers, as a CI artifact and a Reports page. The decision-survival block
    (S22b) runs with it, next to a plain-CLI arm that compacts on its own (L9).
  - A weekly sample is drawn from real lineages.
  - A regression becomes a GitHub issue, never a hold. Cold-return cost is reported, never a gate (S24).
- **Done when:** the first nightly artifact exists.

**S26. Codex parity.** Codex.
- **Scope:**
  - The bootstrap on Codex (the standing instructions already reach it, S17).
  - The divider when Codex compacted, read from the rollout.
  - The 1M window: the S22 probe held, capped at the model's 872k maximum (Codex compacts at about 90% of
    it). Set `model_context_window` and `model_auto_compact_token_limit` per model from its
    `max_context_window`, and seam below the compaction point.
  - No Codex hooks.
- **Done when:** a Codex dev session goes through a debug-threshold seam with its divider.

**S27. Tool-output trimming, A/B first (Q8).** Opus, later.
- **Scope:** an A/B on whole tasks of a lower `bashOutputMaxChars` and of the exact-read instructions. Switch on
  only with ≥5% measured saving on whole tasks and no quality loss.
- **Done when:** the A/B report exists. Turning it on is a separate one-line change.

### Stage 6: restore the grill

The user's answers of 10-23 to the w95 audit (§9.6). Each step's goal is the user's answer and carries its tag;
the how is proposed unless tagged. Steps on disjoint files carry ∥.

**S28. Chats are sessions: workers allowed, the CLI's own shell (w95 items 1, 20).** Opus.
(user, 2026-10-23: "a chat is always a brigade a chat acts almost exactly as a normal session")
- **Scope:**
  - `brigadier start` from a Chat starts a worker of that Chat, shown in its rail, Workers row and worker cards.
    The refusal "only a session with a project starts workers" goes (`plain_workers.rs`).
  - Where a Chat's worker runs (proposed): `--local <path>` or `--worktree` against a repo the Chat names with
    `--repo <path>`; with no repo, a folder of its own under the Chat's session folder, with no git.
  - A worker still can't start workers; it uses its CLI's own subagents (user, 2026-10-23: "workers cant start
    other workers").
  - The Chat's CLI gets its own shell like a session: cwd = the Chat's session folder, the permission level
    mapped as S6 maps a session's. On Claude, no `dontAsk` and no `Bash(brigadier:*)`-only rule; on Codex, no
    read-only sandbox of Brigadier's choosing, and Brigadier approves or declines no command (the heredoc
    refusal goes too). Side chats keep S16c's limited shell.
  - The `/brigade` skill's "a Chat can't start workers" line goes (`brigade/SKILL.md:22`).
- **Files:** `M/plain_workers.rs`, `M/brigade/SKILL.md`, the Chat launch in `P/claude/mod.rs` and
  `P/codex/mod.rs`, `P/chat_shell_live_tests.rs`, `M/prompts.rs` (Chat text, if any names the limit).
- **Done when:**
  - A flow test: a Chat's `brigadier start` makes a worker that shows in the Chat's rail and in
    `brigadier workers`; a worker's own `start` is still refused.
  - Provider tests: a Chat's Claude flags and a Chat's Codex sandbox and approval policy equal a session's at
    the same level, with the session folder as cwd.
  - `git grep -n "a Chat has no repository\|a Chat without one can't start"` finds nothing.
  - A live dev Chat asked for two workers runs them (w86's refused case).

**S29. The parent is told about its workers, the Delegator way (w95 item 8, Q9, O1).** Opus.
(user, 2026-10-23: "The parent should know everything about it's children!")
- **Scope:**
  - **Durable events.** Worker events (turn end with its last message, needs input, failed, interrupted, gone,
    usage limit, fresh context) go to a per-parent queue with cursors, `record/worker-events.jsonl`, so none is
    lost across a daemon restart or a parent seam (O1, §9.1's bar).
  - **Claude, the Delegator way.** `brigadier start` answers with one line: keep `brigadier wait` running in
    the background. The parent runs it as a background Bash task; when an event arrives, `wait` exits, and
    Claude Code re-invokes the parent with the task's result, idle or not, as it does for `dlg wait`. That is a
    result of a command the thread started (G1).
  - **Codex equivalent.** Probe first whether a finished background terminal re-invokes an idle Codex thread
    over app-server. If it does, the same as Claude. If not, Brigadier starts the idle parent's next turn with
    the event batch, and queues it into a running turn like `brigadier send` (Q9).
  - **Safety net (Q9).** When a parent's turn has ended and no `wait` of its own is running, Brigadier sends
    one batched message, "Worker *title*: finished · *excerpt*" (or asks, stopped, failed), never twice.
  - The skill's "workers send you nothing on their own" line becomes: events reach you; keep a `wait` running.
- **Files:** a new `M/worker_events.rs`, `M/plain_workers.rs`, `crates/cli` (`wait`), the turn start in
  `M/conversation.rs`, `P/codex/mod.rs` (turn start or steer), `M/brigade/SKILL.md`.
- **Done when:**
  - A flow test with the fake CLI: a parent ends its turn with a background `wait` running; a worker finishes;
    the parent's next turn starts with the event, once.
  - A flow test: no `wait` running; a worker asks a question; the parent gets one batched message, once.
  - A flow test: an event survives a daemon restart and is delivered once after it.
  - The Codex probe is written up in the step's report; a live dev check on each CLI shows a parent woken by
    a finished worker.
- **Probe (10-23), as built:**
  - Claude Code 2.1.309 (`-p`, stream-json in and out): an idle session **is** re-invoked. Its turn ended at
    6.1 s; the background Bash task ended at 29.8 s; Claude sent `task_updated` (completed), `task_notification`
    (with the output file), `background_tasks_changed []`, a new `init`, then a new turn and `result` at
    31.8 s.
  - Codex 0.163.0 (`app-server`): an idle thread **is not** re-invoked. Its turn completed at 8.6 s; the
    background command's `item/completed` came at 30.6 s, and no `turn/started` followed through 120 s. So
    Brigadier's lingering-command message (`[your background command … finished after your turn]`) starts
    the turn, now for a Chat as well as a session. The skill tells both CLIs the same: keep `wait` running in
    the background.
  - Each event has an id (`ev-…`) and is held by one holder (a `wait`, or the net's message `wn-…`) until
    heard: a wait's hold is done once its `brigadier` printed the events and said so (`PROTOCOL` 2's `Heard`
    frame; a client that hangs up first frees them), the net's once the parent's CLI took the turn. After a
    restart wait holds are freed, and a net hold is done if the parent's own CLI session file holds the
    message, else freed. The net waits 1.5 s (O1's ≤ 2 s) and sends nothing while a `wait` runs or the parent
    works. `failed` and `interrupted` are event kinds (no `status` field); `quiet` is env-settable
    (`BRIGADIER_WORKER_QUIET_SECS`) and never fires while the worker waits on a permission prompt.

**S30. Sessions message each other and read every project (w95 item 12).** Opus.
(user, 2026-10-23: "Sessions should be able to message eachother when asked")
- **Scope:**
  - `brigadier send <session> <text>` reaches another sidebar session, in any project, queued mid-turn like a
    worker message. The recipient's thread shows it as a message from that session (its title, openable),
    never as the user's.
  - `brigadier wait <session>` waits for that session's turn end, with its last message.
  - The skill and `brigadier send --help` say when: when the user asks, or when it is absolutely needed (the
    user's words). Nothing in code limits it.
  - "Read anything" is not limited to the current project: `brigadier sessions --all` lists every project's
    sessions; `show` and `read` take any session id (w95 item 12).
  - Another session's workers stay out of reach: `send`, `wait` and `close` on them are refused (G19).
- **Files:** `crates/cli` (`sessions`, `send`, `wait`, schemas), the daemon's `brigadier` handler,
  `M/plain_workers.rs` (targets), the thread item for a session's message in `D/app/conversation/`,
  `M/brigade/SKILL.md`.
- **Done when:**
  - A flow test: session A sends B a message mid-turn; B's thread shows it as from A; B's `send` back reaches
    A; `brigadier wait <B>` returns B's turn end.
  - A flow test: `brigadier sessions --all` lists a session of another project, and `read` returns its last
    message.
  - A flow test: `send` to another session's worker is refused.
  - The `--json` schema tests cover the new targets.
- After S29 (shared `crates/cli` and `plain_workers.rs`).
- **As built:** a message gets an id (`sm-…`) and is kept in the recipient's
  `record/session-messages.jsonl` before anything sends it; it is steered into a running turn, else queued once
  (by its id) and starts a turn. It is seen once its CLI took it: the steer was accepted, or the turn carrying it
  started. After a restart a message not seen is seen if the recipient's own CLI session files hold its id in an
  input record, and is sent again otherwise (SESSION-MESSAGING-PLAN §durability's three crash points, each a flow
  test). Its CLI gets it under a header, `[message from session "‹title›" (‹id›) · sm-…]`, which names whom to
  answer; the thread draws it with the existing message-from-a-session bubble. Chats count as sessions:
  `sessions --all` lists every project's sidebar sessions and every Chat, with its project. `wait <session>`
  returns once the session rests, took everything the caller sent it, and a turn ended after that; it takes
  sessions or workers, not both. A worker can't message a session. Known gap: Claude may take a steered message
  only after the running turn's result, so a `wait` can return that turn's end.

**S31. No `tmp/` sweep (w95 item 13; #532).** Codex.
(user, 2026-10-23: the grill's design stands: `tmp/` is a plain `TMPDIR`)
- **Scope:** delete the per-turn sweep: nothing empties `tmp/` when a session's requests end, and no
  left-behind process is killed (`thread_scratch.rs` `sweep_scratch_when_over`, the launch sweep and the
  process reaping). Only Archive and Delete clean `tmp/` (S20). Settings › Storage's Free up space keeps only
  what the user starts by a click (proposed).
- **Files:** `M/thread_scratch.rs`, its call sites in `M/conversation.rs` and `M/mod.rs`,
  `M/flow/thread_scratch_tests.rs`, `M/disk/mod.rs`.
- **Done when:**
  - A flow test: a file in `tmp/` and a background `sleep 600` started by the session both survive the turn's
    end and a daemon restart.
  - A flow test: Archive removes `tmp/`.
- **Closes:** #532. ∥ S28–S30, S32–S36.
- **As built (verifier, 10-23):** the CLI process layer's reaping goes for a session's and a Chat's CLI too
  (`SessionSpec::keeps_detached`): when its CLI ends, only the CLI's own process group ends with it; nothing
  it left is tracked, recorded for the crash sweep or swept from its folder. Workers, side chats and tab chats
  keep "nothing survives". Archive and Delete end what runs in the session folder.

**S32. Settings › Routing comes back as the rules editor (w95 item 14).** Opus.
(user, 2026-10-23: "user should've been able to apply his rules there"; D22: "routing becomes advice not a
gate, but we should still fucking let the user interactively add his rules, pick which model is best at what, or
refresh the routing for automatic choosing, which all these then go as the \"advice\" to the brigadier cli")
- **D22's part:** the page also lets the user pick which model is best at what (the model-fit rows) and refresh
  the routing for automatic choosing (Refresh re-reads each CLI's models and the shipped guidance into those
  rows). All of it is advice `brigadier models` and `brigadier start` read; nothing becomes a gate (G18).
- **Scope:**
  - Settings › Routing returns under Agents. It edits the same file `brigadier models` reads,
    `~/.brigadier/models.md`: the user's preferences (Work / Agent / Model / Effort / Reason rows) and rules
    (forbid, effort ceiling, allowlists), beside the shipped guidance, read only.
  - A hand edit of the file shows when the page opens again; the page writes the same text format.
  - The look is the user's Routing design as of `ec0e74408^` (S16b deleted it; `44c718005`), re-fed from the
    rules file; any change from it gets A/B pictures first (proposed).
  - `brigadier start` without `--model` or `--effort` takes them from the rules: the user's row for `--work
    <kind>` (the file's Work key), else the shipped row; its receipt says which row picked them. The skill
    passes `--work` (user, 2026-10-23: "the brigadier cli would pass the --model").
- **Files:** `D/app/settings/routing/*` (restored), the Settings nav, a daemon read and write of the rules file
  in `M/models.rs`, `crates/cli` (`start`), `M/plain_workers.rs`, `M/brigade/SKILL.md`.
- **Done when:**
  - App tests: the page lists the user's rows and rules from a fixture file, and a saved edit writes the file.
  - A flow test: a rule written through the page shows in `brigadier models` and refuses in `brigadier start`.
  - A flow test: `brigadier start --work <kind>` with no `--model` starts the row's model and effort, and
    the receipt names the row.
- After S33 (shared `M/models.rs`) and S28 (shared `plain_workers.rs`).
- **As built (follow-up, 10-23):** the five foundations.pen Routing rows (`jlV2S`, `vqDc3`, `QlVJ4`, `AL8pa`,
  `v2biKX`) on the four kinds of work. The file format grows two things:
  - Several rows of one Work are an order (Automatic / My order on the page): the first row the rules allow
    takes `brigadier start --work`, else the first, which is then refused. A Work's rows replace its shipped rows.
  - A rule may name one Work: `forbid for <Work>: …`, `allow <agent> for <Work>: …`, `effort-ceiling for <Work>:
    …`. It judges only `brigadier start --work <Work>`; chats, sessions and `brigadier models`' list ignore it.
  - Each rule row says its scope and effect ("Everywhere · Chats, sessions and workers can't run on it",
    "<Work> only · Its workers can't run on it"); a draft that would refuse a kind names it with an Open link.
  - The page calls the shipped advice Automatic. How a model is picked lists the four true steps (your choice,
    your order, Automatic, your rules, which win over all) and ends on an example from the page's own kinds.
  - Not built (G18, no router): the pen's Prefer effect, scores, Explanation view, Route sections, 7 steps.

**S33. A neutral shipped rules file (w95 item 16).** Codex.
(user, 2026-10-23: the shipped default must not carry the user's personal rules)
- **Scope:** `M/models_default.md` forbids nothing and sets no ceiling: only its comments. A rules file that
  exists is never overwritten, so this user's own file keeps never Fable and effort ≤ high. Tests write the
  rules they need.
- **Files:** `M/models_default.md`, `M/models.rs` and its tests, the S13b flow test.
- **Done when:**
  - A unit test: the shipped default parses to no rules.
  - A flow test: on a fresh home, `brigadier start` with any model the CLI offers is not refused; an existing
    rules file is left byte for byte.
- ∥ S28–S31, S34–S36.

**S34. PR row words and colours (w95 item 6).** Codex.
(user, 2026-10-23: "fix this")
- **Scope:**
  - The grill's list, exactly: Draft (grey), Checks running (yellow), Review needed (yellow), Ready (green),
    Merged (purple), Closed (grey), Checks failed (red). The build's own Open, Conflicts and Changes requested
    go.
  - Which word wins (proposed): Merged, Closed, Draft, Checks failed, Checks running, Ready (approved, checks
    green, mergeable), else Review needed (changes requested and conflicts included).
  - Refresh after `git push` and `gh pr` commands, plus a light poll while the session is open (proposed:
    every 60 s while its tab is open and the PR is neither Merged nor Closed).
  - Without a `gh` sign-in the row says "gh auth login for PR status".
  - Yellow and purple come from the design system's tokens; a missing one goes into `foundations.pen` first.
- **Files:** `D/app/conversation/pullRequestWords.ts`, `SummaryContent.tsx`, `M/pull_request.rs`.
- **Done when:**
  - An app test per word and colour.
  - A flow test: with a `gh` stub, the poll turns Review needed into Ready with no push.
  - A flow test: a signed-out `gh` stub gives the sign-in hint.
- ∥ S28–S33, S35, S36.

**S35. Computer use reaches sessions (w95 item 5; #529).** Opus.
(user, 2026-10-23: "fix this!")
- **Scope:** #529's fix. While Settings › Computer use is on, every session, Chat and worker on both CLIs gets
  the vendored driver's own MCP server and its skill (`computer_use_kit`), as a server entry of its own beside
  the user's. A lean worker gets it only when the thread asks (proposed). Brigadier's own MCP server stays gone
  (G16). The MCP aliases and `MCP_TOOL_TIMEOUT` come back in `session_env` for this server only.
- **Files:** `M/computer_use.rs`, `M/computer_host.rs`, the session and Chat launch in `P/claude/mod.rs` and
  `P/codex/mod.rs`, `D/app/settings/ComputerUsePage.tsx` (wording only).
- **Done when:**
  - A thread and a Chat on Claude and on Codex list the computer tools when the setting is on and none when it
    is off (provider tests).
  - A live check drives a background app through them.
- **Closes:** #529. After S28 (shared launch code in the providers).

**S36. Model fusion when the user is uncertain (w95 item 3).** Opus.
(user, 2026-10-23: "we do a model fusion and give the best advice/answer")
- **Scope** (the design is proposed):
  - **When:** the user types `/fusion <question>`, or the thread judges the user uncertain (they say they are
    unsure, or ask which of several ways to take). Not for every reply.
  - **Which models:** one consult per installed CLI (Claude Code and Codex today, more CLIs later), each on the
    strongest model and effort the user's rules allow and with quota left, read from `brigadier models`. A
    `fusion` row in the rules file (S32) can name them.
  - **How:** the thread starts them as lean workers (`brigadier start --lean`) with the same brief: the
    question, the context it gathered (file:line, the ledger's rules and decisions that bear on it), and
    "answer only; change no file". Each answers alone: a recommendation, its reasons, risks and confidence.
    Then one cross round: each reads the other's answer and says what it would change (adversarial, G23). No
    further rounds.
  - **Merge:** the thread writes one answer: the recommendation; where the models agreed; where they differed,
    which side it takes and why; what would change the call. The thread judges; it never counts votes.
  - **Where it shows:** the answer is the thread's own reply, naming the models consulted. The consults show as
    the thread's workers ("Fusion · *model*"), readable like any worker. No new card.
  - `/fusion` ships as a second skill, `fusion`, beside `/brigade`, through the same plugin folder on both CLIs
    (Q13), so S15's "the one skill Brigadier ships" becomes two. The old review `M/fusion.rs` (gone in S5) is
    not revived.
- **Files:** a new `crates/core/src/manager/fusion/SKILL.md`, the skill installer (`brigade.rs`),
  `M/brigade/SKILL.md` (the uncertain-user line, S15), the S13b guidance file (a `fusion` row).
- **Done when:**
  - A provider test: every new session and Chat, on both CLIs, has the `fusion` skill in its skills folder.
  - A live dev run: `/fusion` on a real two-way question starts one Claude and one Codex consult, and the reply
    names both models, what they agreed on, where they differed and the pick.
  - A live dev run: "I'm not sure whether A or B" without `/fusion` leads the thread to a fusion. Judgement,
    so it is a report, never a gate.
- After S30 (shared skill file).
- **As built:** the rows are per CLI, since a rules row holds one Agent/Model/Effort and is keyed by its Work:
  the shipped guidance has `fusion-claude` (claude · opus · high) and `fusion-codex` (codex · default · high); the
  user's rows of the same Work replace them, and another CLI gets `fusion-<agent>`. The skill starts each consult
  with `brigadier start --lean --work fusion-<agent> --title "Fusion · <model>"`, overriding `--model` only when
  that model is out of quota or refused. `fusion/SKILL.md` ships in the brigade plugin folder beside `brigade`
  (`skills/fusion/SKILL.md`): Claude loads both from the plugin, Codex adds the plugin's skills folder as a per-app-server skills root. It
  has no user copy of its own. `brigade/SKILL.md` sends an uncertain user to it. Live dev runs (10-23, an Opus parent): `/fusion` on a two-way question started a lean Claude consult (opus) and a
  lean Codex consult (its default model), ran the cross round, and replied with the pick, what they agreed on,
  where they differed and the models consulted. "I'm not sure whether A or B" without `/fusion` led the thread to
  the same fusion. With `brigadier` unusable (an older CLI on `PATH`), the thread ran the two CLIs' one-off modes
  itself and still merged.

**S37. Hooks parity (w95 item 15).** Codex.
(Delegator's judgement, 2026-10-23: parity wins; Brigadier stops overriding disableAllHooks).
- **Scope:** delete the `disableAllHooks: false` override. A user who disabled all hooks gets none inside
  Brigadier, including Brigadier's own. Read the CLI's own event stream for hook-backed features (G2); rebuild
  no hook layer. If the stream cannot provide a feature, degrade silently and record exactly why in a GitHub
  issue here; never prompt the user to turn hooks on. With hooks enabled or unset, keep today's behaviour.
- **Stream behaviour:** command results, saved-output paths/excerpts, usage, background tasks, turn completion
  and compaction use the existing CLI stream parser. There is no Brigadier `UserPromptSubmit` hook. The worker
  launch already supplies no `Stop` hook: completion comes from the CLI's result, with no report gate.
- **Hookless gaps:** [#539](https://github.com/syncra-studio/brigadier-ai/issues/539). A later stream event cannot
  rewrite the model's Bash result into Brigadier's custom digest/`out-*` alias, block an already completed turn,
  or veto recovery compaction. The CLI's own saved output remains readable. `DISABLE_AUTO_COMPACT=1`, idle
  compaction off and the usage-driven seam stay (switch off; with `BRIGADIER_LONG_SESSIONS` on, see G12: no
  `DISABLE_AUTO_COMPACT`, no size seam, idle compaction still off, run2-D12, and a one-time notice that the
  decisions aren't put back, run2-D16); a real compaction is reported as such, never as a refused
  hook. A note fork with hooks off uses the CLI's empty built-in tool list and strict empty MCP config: still
  read-only, but the changed tool schema loses its shared cache prefix. These gaps degrade silently; no hook
  replacement or request to enable hooks.
- **Files:** `P/claude/mod.rs`, its settings-layer tests, this plan.
- **Done when:** a provider test starts Claude with the user's `disableAllHooks: true`, no override and no
  Brigadier hooks; false and absent keep today's settings. Existing feature tests pass with hooks off; list
  those tests and each hook-dependent feature in the report. Workspace clippy is clean.
- ∥ anything; rebase after S22/S23 land if they touch the same hook code.

**S38. Account auto-switch and the provider fallback (D23).** Opus.
(user, 2026-10-23, D23: "that auto-switch, it was for auto-switching automatically between provider accounts, for
example if all the claude quota is done, it should fallback to another provider if enabled, and then inside the
actual provider page, we should have the account auto switch toggle as well, where user can pick which one to use
now (the auto switch toggle goes to disabled) and if enabled it should prioritize and use the correct account as
we discussed that the one which is soon to be hitting the usage limit should be used first and then go to the
other one, it should smartly use the correct accounts based on their reset times, usage limits, weekly limits, 5
hour limits, weekly reset time")
**Take, don't rebuild:** the account switching Brigadier has (`switch_account` continues the same CLI session on
a limit, D14; the per-agent "Switch accounts automatically" row and Use in `D/app/settings/providers/AgentPage.tsx`;
`crates/core/src/accounts.rs`), MA1's limit signals and the seam (S22, S23). No router comes back: the user's
toggles decide, the engine switches accounts, and the provider fallback is a seam to the other CLI. The how
below is proposed unless tagged.
- **Scope, 2a: the account toggle, pick-now and the smart order.**
  - **Per provider, on its page:** the "Switch accounts automatically" row stays where it is (D23). Use on an
    account turns it off and makes that account the one to use now (D23: "the auto switch toggle goes to
    disabled"). The pick is immediate: new sessions start there, and a running session moves at its next
    request, through `switch_account`, with the moment "*CLI* continues on *account*, picked in Settings" (as
    landed: not Q12's "carry on" line, since nothing was interrupted; it acts only when the pick changed since the
    conversation's last turn, so a per-chat account choice stays).
  - **Off:** work stays on the picked account. Its limit still moves work to another account of the same
    provider before anything else, as built (#505; proposed: kept, since D23's fallback starts only once all of
    a provider's quota is done).
  - **On, the order (proposed):** an account can take work when it is signed in, has hit no limit, and no window
    that applies to the model (5-hour, weekly, per-model) is used up. Among those, the account whose weekly
    window resets soonest goes first, counted in whole days from now, because its quota is lost first; within the
    same day, the account nearest to its limit goes first (the least left in its tightest window, 5-hour or
    weekly), so it drains and the other stays whole; then the earliest added. An unknown quota sorts last. An
    account in a limit waits for its reset (MA1's `resets_at_ms`, else the window's reset) and rejoins the order
    then. A running session keeps its account until that account is out (a switch re-reads the whole context
    uncached); new work and every switch follow the order. Usage and reset times come only from each CLI's own
    signals (MA1, §9.2). The Delegator's own rule (switch at 99–100% of a 5-hour window, go back at its reset)
    is the hint for this order, not a product rule.
  - **The page shows the order, read only:** with the toggle on, one line per account under the toggle, in
    order: its place and the reason ("1 · weekly resets Fri, 82% used", "2 · 5-hour limit until 14:10"). Off, the
    picked account reads "In use, picked". It reuses the page's existing rows (no new look).
  - **`brigadier models --json`:** per provider `switch_accounts` (bool) and the account `order` (account id,
    place, reason, `ready_at` when in a limit); top level `switch_providers` (bool). The text view adds one
    line per provider. Advice only (G18).
- **Scope, 2b: "Switch to another agent automatically" (D23).**
  - The row S16b removed (`ec0e74408`, `AgentsOverview.tsx` "Fallback" section, setting `switchProviders`)
    comes back where it was, with its words, on the Providers overview.
  - **On:** when every account of a provider is out (none can take work by the order above), a running session
    hands off through the seam to the other installed, enabled CLI: the same thread, the divider "Fresh context ·
    hand-off note" with "Usage limit on *provider* · continued on *other*", the note, and the successor carrying
    on at once (S22, S23, D8). Its model and effort come from the rules file's `Fallback` row (S32) when that row
    names the other CLI, else the other CLI's default, else each model it lists (as landed): the first the user's
    rules allow through `brigadier start`'s own refusal; none allowed → the session waits as with the switch off
    (D25). No extra moment line: the divider says why.
    When the out provider resets, the session stays where it is (as landed: a switch back would be a second
    seam). Sessions only: a Chat has no seam and keeps Q11.
  - **Off:** Q11, as built: the "Resumes at" card, then the same session continues at the reset.
  - A death with no note (the limit hit mid-note) uses S22's "start from the ledger and tail" path.
  - A new session takes the same path at its first limit. Turning the switch on while a session waits for a
    reset moves it at once. `brigadier start` picks nothing itself: `brigadier models` shows each account's state
    and `switchProviders`, and the caller chooses (as landed; advice, never a gate, G18).
  - No env flag (as landed): the switch is off by default (D23: "if enabled") and is the gate.
- **Files:** `crates/core/src/accounts.rs` (the order and its reasons), `crates/core/src/model.rs` (`switch_providers`
  back), `M/conversation.rs` (pick-now at the next request, the out-provider branch), `M/models.rs` and its schema
  (`order`, the two toggles), `D/app/settings/providers/{AgentPage,AgentsOverview,AccountRows,ProvidersPage}.tsx`,
  their render tests, the seam module (2b).
- **Done when:**
  - App tests: the provider page's toggle, Use turning it off and picking the account, the order lines with
    reasons; the overview's "Switch to another agent automatically" row reads and writes `switchProviders`.
  - Unit tests of the order: weekly reset day first; within a day the nearer-to-limit first; a 5-hour limit
    waits for its reset and rejoins; unknown quota last.
  - A flow test with fake CLIs: two Claude accounts with different 5-hour usage and resets → the nearer-to-limit
    one takes the work; a limit on it switches to the other with one "carry on" line, the same CLI session id;
    with both out the session waits with the "Resumes at" card (fallback off) or seams to Codex with the divider
    and the note (fallback on).
  - A flow test: Use on a running session's provider moves its next request to the picked account.
  - `brigadier models --json` shows the order and both toggles (schema test).
- **Order:** 2a ∥ anything (rebase after S32 if it touches `M/models.rs` first). 2b after S23 lands (Stage 5,
  w108); it rebases on it.

**S38c. "Continue on <other CLI>" on request (D46).** Opus. (as landed)
D49 amendment (10-24, S22b/S38c, extends D46/D23; D25 applies): request/quota seams carry the predecessor's hot context up to about half the target's known window after the whole S22b sections; unknown windows keep the tail, size seams keep ≈15k tokens, and omitted spans keep their reads.
(user, 2026-10-23 23:46, D46: "i guess sure, why not", to: should a session move to the other CLI when you ask,
the same seam, divider and hand-off note as the quota fallback, or only when quota runs out?)
**Take, don't rebuild:** S38 2b's seam (`seam_to_agent`), S22's nudge and S22b's bootstrap, D25's model pick
(`successor_model`, shared with `fallback_agent`). No router, no new mechanism; D23's switch is unchanged, and a
model or account change within one CLI still continues the same session (D14).
- **Ask:** `brigadier continue-on <claude|codex> [--after-turn]` from the session's own thread, or the composer's
  model picker: the other CLI's list opens with "Continue on Codex" ("Fresh context, with a hand-off note"), and
  picking one of its models moves there on that model (IPC `continueOn`). The successor runs on the named model,
  else the first the user's rules allow there (D25).
- **How:** the request is kept in `record/seam.json` (`moving`) until no turn runs. The CLI it runs on still
  works: the seam's own nudge asks it for the note, and the seam that closes moves the session's model, so the
  successor starts there from the note at once. The CLI it runs on is at its limit: it moves at once without a
  note, as 2b (the divider: "usage limit on … · continued on …"). A limit in the turn that asked goes there too.
  The divider reads "Fresh context · hand-off note · continued on Codex" (`SeamMark.movedTo`).
- **Refused** (exit 3, plain words): a turn runs without `--after-turn` (the app always passes it); a worker
  ("moved by its session"); that CLI turned off, not signed in, or every account at its limit; the rules allow
  no model there (the rule's words); a Chat; a hand-off already under way. Asked for the CLI it runs on:
  "already", and a pending move is dropped.
- **Skill:** no line: the brigade skill is at its byte budget; `brigadier --help` lists the command.

**S39. `brigadier why <file>:<line>` (D51).** Opus.
(user, 2026-10-24, D51: "sure, do the brigadier why", answering: make "why was this line written" (`brigadier why
<file>:<line>`: walks the per-turn snapshot refs to the turn that wrote it, prints that turn's message, reply and
ledger entries from the CLI's files; no new store) a plan step now.)
**Take, don't rebuild:** the CLIs' own session files (G11, `record/lineage.json`), the per-turn snapshot refs
(S11, `refs/brigadier/s/<id>/turn-<n>-{start,end}` and their `-head` anchors), the request → turn number records
S11 already keeps, and the ledger (S17, S22b). No new store, no new recording, nothing leaves the machine. Pull-only:
nothing is pushed into a thread. The how below is proposed.
- **Scope:**
  - **Input:** `brigadier why <file>:<line>`, or `<file> --line N`; the path is relative to the caller's current
    folder (any checkout of any project). Read-only, for Claude and Codex sessions alike.
  - **The walk (blame-like):** the sessions whose checkout is this one, seam successors included (one id, one
    ref namespace), their turns ordered newest first. From the line as the file has it now, each step maps it
    back: across the gap after a turn (end tree → the next state), then through the turn (start → end tree). The
    first turn whose own diff added the line's current content wrote it. A line the turn only moved (the same
    text deleted elsewhere in that turn, in any file) is followed to where it came from, and the walk goes on. A
    line added in a gap, or older than every turn, was written outside Brigadier: `git blame` names the commit,
    or "not committed".
  - **Workers (G21):** a worker has no turn refs of its own; its work lands in the parent's turn by a merge or a
    fast-forward. When the line came in that way (`git blame` at the turn's end head names a commit made during the
    turn, off the session's own first-parent line, or on a worker's branch), the answer names the worker and reads
    its task and last reply from its own CLI files.
  - **Output, plain and short:** the line; session name and id; turn number and time; the user's message for
    that turn, whole; the reply, trimmed, with the exact `brigadier brain read <session> <from> <to>` for all of
    it; the ledger entries stamped inside the turn, by id. A session whose CLI file is gone (the CLI cleaned it
    up, or the session was deleted) says so plainly and prints what git still knows. `--json` for machines.
  - The `brigadier` help lists it; the brigade skill's command table gets it only if it fits in one line within
    the skill's byte cap.
- **Files:** a new `crates/git/src/why.rs` (the walk, its line map, the blame fallback), `crates/cli`
  (`commands.rs`, `args.rs`, `protocol.rs`, `render.rs`, `schemas/why.schema.json`), a new `M/cli_why.rs` (the
  sessions of a checkout, turn ↔ request ↔ the CLI's turn, the reply and ledger read), `M/cli.rs` (dispatch),
  `M/brigade/SKILL.md` (one line, if it fits).
- **Done when:**
  - Unit tests of the walk on a scratch repo with fabricated snapshot refs: a line one turn introduced; a line a
    later turn modified (the later turn wins); a line a later turn moved (the first turn still wins); a line from
    the user's own commit (`git blame`'s commit); an uncommitted edit outside any turn.
  - A flow test with the fake CLI: two turns edit one file; `why` on a line of each returns that turn's message,
    reply and ledger entry; with the CLI file removed it says the file is gone.
  - A live dev run: one real Claude session edits a file over two turns; `brigadier why` on a line from each
    returns the right turn's message (Codex too, if quick).
- **Order:** ∥ anything. Sources: D51; the run's w156 report (P2) and w160 report (rows 12 and 15), from Delta's
  "immortal" idea, done locally.

### Order and parallel runs, in one view

```
S1 → S2 → S3 → S4 → S5 → S6 → S7 ─┬→ S8 → S9 → S10 → S12 → S13 → S13b → S14 → S15 → S16a → S16b → S16c
                                  └→ S11 (∥ S7–S10; merged files go under worker cards once S13 is on)
after S10:  S17 → S18 → S19 → S20      (∥ Stage 3; S17 rebases after S16)
after S8:  S21 (∥ anything)
after S18 + S21:  S22 → S22b → S23 → S25 → S26 → S27      (S24 dropped, decided 2026-10-23)
Stage 6, now (user, 2026-10-23):  S28 → S29 → S30 → S36 · S28 → S35 · S33 → S32 (after S28)
                                  ∥ S31, S33, S34, S37 with anything (S37 rebases after S22/S23 hook changes)
                                  S38: 2a ∥ anything; 2b after S23 (D23) · S39 ∥ anything (D51)
any time:  MA1, X1 (bounded Codex tasks, §9)
after Stage 2, one run per capability (§9):  O1–O5 with S13–S15 · MA2–MA7 with S13–S14 and S23
                                             L1–L9 with S18–S27 · BR1–BR8 after S13 · X2–X9 after X1
```

## 5. Benchmarks never gate

The continuity battery (S21, S25), the token benches and the A/B against cmux `/delegator` (S15) are reports.
They run nightly or before a release. A regression becomes a GitHub issue. No step's "done when" waits on a
number from them, except S23's switch-on, which compares the seam with today's rebirth. That is the user's Q6
decision, and the comparison is read by the person landing it, not by a gate in code.

Outcome learning (which model finished what) comes only later, as a nightly report the user reads. It never
changes the guidance or the user's rules file by itself (G18).

## 6. Standing instructions (draft of `standing_instructions.md`)

This file is all Brigadier adds to a session (G17). The session still gets everything the plain CLI loads: the
project's CLAUDE.md or AGENTS.md, the user's MCP servers, skills, and memory per the user's own settings.
Brigadier removes none of it; a `--lean` research worker (S13) is the thread's own choice (G23). The file
holds only what every turn of every session needs; text that matters only at a seam travels in the seam
message instead (`research/POCOCK.md` §3.1). The entry ids, the Decision and Open kinds, absolute paths and the
"cites entry ids" line were added on 10-23 (S22b, `research/CONTINUITY.md` §3.1–§3.6) (user, 2026-10-23: "find a fix for this").
A worker gets the file without its last line, the orchestrator line: `brigadier start` refuses a worker's own
workers, it has no brigade skill (the w152 bench, 10-23), and it keeps full reports (run2-D27). That line makes
the session an orchestrator with the short reply, with no subagent split (run2-D23, run2-D24, run2-D27,
run2-D43).
D63, D58: appended standing text keeps `$NAME` path placeholders and explains shell expansion; every named variable is exported by the session, so the text is identical across sessions.

```
This is a plain Claude Code / Codex session inside Brigadier.
Your session folder is $BRIGADIER_SESSION_DIR. Scratch goes in $TMPDIR, never the repo.
Keep a ledger at $BRIGADIER_LEDGER. `brigadier ledger add` works in every role except chats beside a tab and stamps each entry with an id and the time; `show` reads it. Write one entry per thing settled (a rule or decision, an open item opened or closed) when it happens, batched with your other tool calls. Never a progress log: not the request restated, a passing check or your own change; a task that settled nothing needs no entry.
Each entry says its kind and source (message, command, file). Kinds:
- Rule from the user: exact words, which message, scope (request, session, project); add "supersedes …".
- Decision: the user's words and what they answered (the option's text, not just "A"), or yours marked "I decided"; keep the sentence that defines it, and the reason.
- Exact identifier (hash, absolute path:line, number with units, id) · Failed check (command, exact error line, fixed or not) · Ruled out (what, why).
- Open: a promise, or a question waiting on the user; `brigadier ledger close <id>` when it is done.
- Learned: file:line and a one-line finding, never a copy.
Never delete entries; move finished ones to ledger-archive.md beside it. A worker's claim is not a fact until you check it; say whose it is.
A plan or brief built from the ledger cites entry ids; mark lines of your own "proposed". Tell the user each "I decided" that changes what they asked.
Read exact ranges. Brigadier's commands: `brigadier --help`; ask the user with `brigadier ask`.
You orchestrate: talk, plan, look things up, make small edits (a few lines), run git and CI, check workers' evidence; never build or test. Bigger work, builds, tests and research go to workers (`brigadier start`; the brigade skill says how). After a block of work reply in 3-5 short lines (what changed, what's next, what needs the user): no recap, headings or filler.
```

**Seam text (not standing).** S22 sends the first part as the seam nudge; the successor's bootstrap carries the
part after `---`. The check, take-over and "raise the oldest question" lines are S22b's (`research/CONTINUITY.md`
§3.3–§3.4).

```
A fresh session will continue this work. Finish the step you are on, at a boundary you choose; don't start new work.
Update the ledger. Run `brigadier ledger show --open --rules`: close or carry each open item by id, and check your last steps kept each rule; write any slip as a Failed check.
Then write the hand-off note at <path>: doing now, waiting for, next, "read A,B,C; next D,E", stale ledger entries. Don't restate rules or decisions; they travel verbatim. Absolute paths only. Then stop.
If this is your second hand-off in a row that is still reading, say so in the note and propose splitting the work.
---
You continue an earlier session. The open items, rules and decisions below are verbatim and authoritative; so are the note and last exchanges.
First add one ledger entry: "Took over gen <n> · open <ids> · applies next: <id> (how), …". If a question waits on the user, raise the oldest one in this reply.
For anything else from before, search the record (`brigadier brain search`, `brigadier brain read`) and quote what you find; if it isn't there, say so rather than guess.
```

## 7. Order of risk: what proves the plan early

- S1–S6 bring immediate user value: the 10-22 failure modes go away. S2 and S3 switch the CLIs' own long
  commands, background tasks and subagents on first, so S4–S6 only remove things.
- S9 is the riskiest build step, because the CLI file formats are internal (§8, item 1). It runs behind a flag
  and is checked against real files before S10 flips it.
- S15 is the one that proves G3: a team run whose workers land real work into their parent, which lands it on
  the user's branch.
- In Stage 6, S29 and S30 carry the risk: they decide what reaches a thread it didn't start. S29's Codex probe
  comes first; S30's messages always show whose they are (user, 2026-10-23: "The parent should know
  everything"; "Sessions should be able to message eachother").

## 8. Open risks

1. **The CLI session-file formats are internal.** Claude Code says its JSONL "is internal … and changes between
   versions" (EVIDENCE 2.4). Codex rollouts are not a public API either.
   - Mitigation: the reader lives in `P/` next to each adapter, with fixture tests from real files, and it is
     re-checked on every CLI version bump.
   - The live stream stays the source while a turn runs.
2. **Background tasks in `claude -p` stream-json.**
   - Start and end are exposed: `system` `task_started`, `task_progress`, `task_notification` (status
     completed, failed or stopped, with `output_file`) and `background_tasks_changed` (EVIDENCE 2.4). They are
     already parsed at `P/claude/parse.rs:884–903`.
   - Open: there is no control request that backgrounds a running tool. A user message with `priority: "now"`
     moves movable work to the background (v2.1.286+), which is the nearest Ctrl+B. Stopping one task needs a
     probe in S12.
   - For Codex, whether app-server exposes `/ps`-style background terminals needs the same probe. If not,
     Codex rows show only what its events carry.
3. **The macOS 104-byte Unix-socket path.**
   - `~/.brigadier/s/<8>/tmp/` is about 35–45 bytes for typical home paths, so a tool's own socket under it
     usually fits.
   - Claude Code's sandbox makes sockets under `<tmp>/claude-<uid>/…`, and Seatbelt drops socket allowances on
     long roots (`crates/sandbox/src/macos.rs:689–711`). So `CLAUDE_CODE_TMPDIR` stays a short `/tmp` path, a
     G13 exception like shared caches.
   - S7's done-when binds a socket to prove it.
4. **Codex hooks need manual trust by hash** (EVIDENCE 2.5). Nothing in this plan uses a Codex hook. The changes
   card uses git snapshots, the seam uses usage events and a queued message, and Background uses events.
5. **The `brigadier` CLI inside the CLIs' own sandboxes.** Under Ask and Accept edits, Codex runs commands in its
   own `workspace-write` sandbox, which may refuse a connection to the daemon's socket (and so may Claude Code's
   sandbox if the user turns it on). S13 probes this on both CLIs before anything depends on it. If it is refused,
   the fix is the CLI's own setting (an allowed socket or path in its sandbox config), never a Brigadier sandbox.
   As built, Codex keeps network on at Ask and Accept edits so `brigadier` reaches the daemon's socket
   (`P/codex/mod.rs`), where plain Codex keeps it off. That stays (Delegator's judgement with the user's
   trust, 2026-10-23: "do as u thinnk it's best, i'll trust your judgement").
   [#538](https://github.com/syncra-studio/brigadier-ai/issues/538) tracks reaching that one socket with network
   access off, using the CLI's own per-socket exemption if supported.
6. **Repo instructions on Codex.** After S17, Codex reads AGENTS.md along root → cwd only, as plain Codex does.
   A project that has only CLAUDE.md, or rules in a subfolder's AGENTS.md below the cwd, gives a Codex session
   less than `instructions.rs` gives it today. Taken as is ("take, don't rebuild"); a project that wants both
   CLIs to see the same rules keeps them in AGENTS.md.
7. **Skills on Codex.** S15 assumes Codex loads a skill from its skills folder. If the installed Codex can't,
   the `/brigade` skill reaches Codex threads as a file the standing instructions' pointer line names. This is checked in S15.
   The skill drives sessions only through `brigadier` commands, so it is the same text on both CLIs.
8. **No Brigadier sandbox (G5) also removes the "protect" profile.** That profile kept every CLI, Full access
   included, from writing Brigadier's own data folder (`P/cli.rs:259–287`). After S6, a session can delete
   `brigadier.db` like any other file it can reach. That is accepted under G5 and the same as a plain CLI. S6's
   commit says so.
9. **Ledger path.** PROPOSAL.md put it at `~/.brigadier/s/<id>/ledger.md`. G15 later placed the kept record under
   `record/`. This plan uses `record/ledger.md` so Archive keeps it, which follows G15.
10. **Many seams in one long turn on Codex.** A Codex turn can compact before the 200k nudge is read (Q0). This
    is accepted and shown by the divider. The ledger, note and rollout survive it.
11. **Big deletions break tests far away.** S16's sub-steps touch shared types (`board.rs`, `model.rs`, IPC).
    Each sub-step runs `cargo test -p brigadier-core` for the touched modules and `pnpm typecheck`. The full suite
    runs once on the integrated tree, as the Delegator's rule says.
12. **Codex plan-review findings not taken** (10-22, `msgs/w12-plan-review.md`):
    - **#1, migrate old history before the new reader:** not taken. There are no real users yet, so old records
      are neither migrated nor kept readable.
    - **#6, a transition for open sessions' old instructions:** not taken, for the same reason. Sessions open at
      upgrade are archived (S1's done-when, and the "Nothing is migrated" rule in §4).
    - Taken: #2 (S9's ids and `record/turns.jsonl`, with a restart-parity check), #3 (the last-consumers table
      in §4), #4 (S2), #5 (S3), #7 (S17 and item 6), #8 (S19), #9 (S11 and S20).
13. **Stripping context by default is unsupported (G17).** The evidence is `research/LEAN-CONTEXT.md` (§0, §5)
    and `research/POCOCK.md` (§3.1, §3.2).
    - Nobody has measured "strip everything" making agents better. Short, human-written project instructions
      are neutral to slightly positive on correctness and cut runtime; curated skills help a lot (LEAN-CONTEXT
      §0, §5 item 7).
    - The real cost, MCP tool schemas, is already deferred by both CLIs, and only skill descriptions load
      (LEAN-CONTEXT §4).
    - So sessions load what the plain CLI loads. Brigadier keeps only its own additions short (§6) and denies
      only the tools that cannot work in a tab, after S17's one-time measurement (POCOCK §3.2).
    - Not taken: LEAN-CONTEXT §5 item 1's default stripping of memory writes, user-global skills and MCP
      servers. Under G17 these stay as the user set them, and come back only with measured evidence on our own
      tasks. G23 takes them per worker instead: the thread may start a research worker with `--lean` (S13).
      POCOCK §3.5's depth limit on `brigadier start` is not built in either: Brigadier enforces nothing
      of its own (G18); a user who wants one writes it into the rules file. The one built-in limit is that a
      worker can't start workers; it uses its CLI's own subagents (user, 2026-10-23: "workers cant start other
      workers").
14. **A worker's CLI permission prompt under read-only workers (G19.Q6). Resolved by Q8 (user, 10-22).** A
    question can be relayed in words with `brigadier send`; a permission prompt needs allow or deny. So:
    - A worker inherits its parent's permission level. `brigadier start --permission` overrides it.
    - A worker's open CLI permission prompt reaches the parent as a `brigadier wait` "needs input" event. The
      parent answers it with `brigadier send <worker> --allow|--deny`, deciding itself or relaying the user's
      answer, through the CLI's own permission protocol.
    - Until it is answered, it shows under Waiting on you in the parent. S13 builds it.
15. **Seen in T3 Code and not taken** (`research/T3CODE-WORKTREES.md` §4, §6): PR-watch wakeups, because
    Brigadier pushes nothing (G1), and confirmations before pushing to the default branch, because permission
    modes belong to the CLI (G5). The thread may poll its PR with `gh` or a background task.
16. **Decisions survive only as well as the model keeps them (S22b).** Ids come from `brigadier ledger add`;
    entries written with file tools have none, and `ledger check` lists them. The take-over line costs about
    100 tokens per seam: if the battery shows no gain in "applied without being asked", it goes. The derived-brief
    check is a model reading a fixed rubric, so it is a report, not proof.
17. **Hooks follow the CLI's own settings.** S37 removes the `disableAllHooks: false` override and registers no
    Brigadier hooks when the user has disabled them. Hook-backed features read the CLI's own event stream
    instead (G2); an unavailable event degrades silently and is recorded as an issue, never a prompt to turn
    hooks on (Delegator's judgement, 2026-10-23: parity wins; Brigadier stops overriding disableAllHooks).

## 9. Excel at what we keep

The user's strategy (10-22): fewer capabilities, each the best of its kind.
Brigadier keeps five: orchestration, model advice and accounts, browser and previews, skills and MCP, and long
sessions with token economy. Each has a measurable bar against the best rival today, and a bench proves it.

**How §9 runs.**
- **After Stage 2.** Parity first: S1–S12 make "a plain CLI" true. Then one run per capability. A step marked
  "into Sn" is built inside that step and adds to its done-when; orchestration, model advice and long sessions
  fold into Stages 3–4 this way. Browser and skills/MCP are runs of their own, after S13.
- **Two may start earlier,** as bounded Codex tasks: MA1 (the three Claude limit signals we miss) and X1 (the
  plugin-folder checks). Neither touches the strip-down's files.
- **Benchmarks report and never gate (§5).** Each bar is a nightly report; a regression becomes a GitHub issue.
- **Evidence:** run `20261022-0805-take-over-5-sessions`, `research/EXCEL-<capability>.md` (orchestration,
  model-accounts, browser, skills-mcp, long-sessions): §3 design, §4 steps, §5 risks. Not restated here.
- **Not copied, for every capability:** Superset is Elastic License 2.0: ideas only, never its code, text,
  proxy URLs or hosted service. bb, T3 Code and Orca are MIT: designs, not code (a ported snippet carries its
  notice). No Claude Code docs text. No rival's feature names or UI words in the product ("Design Mode",
  "Auto-verify"), and no rival named in the UI or in marketing.

### 9.1 Orchestration

- **Best rival today:** Orca ships the most complete worker loop: start with model and effort, typed waits, and a
  liveness verdict per worker. It sits behind an experimental switch. No rival merges a worker's work back in
  git (`EXCEL-orchestration.md` §1–§2).
- **Our bar:**
  - **Nothing stuck.** The doc's 10 injected faults (prompt, question, error, kill, silent command, seam, idle
    parent…) reach the parent ≤ 2 s after the CLI event; none is lost across an app restart or a parent seam.
  - **Lands.** On a two-worker task (one Claude, one Codex, one shared registry file), ≥ 9 of 10 runs land in
    the parent and then the chosen base, with green checks and no human touch. Every real conflict shows as
    `merge: conflicts` before the merge. 0 uncommitted user files are overwritten.
  - **One skill** of ≤ 150 lines; a user edit survives an app update. **No worker protocol:** a brief with no
    Brigadier text still yields `turn_end` and `needs_input`. **Visible:** a state word changes ≤ 1 s after its
    CLI event, "Merged" ≤ 5 s after the merge.
  - **Lean research.** A research task given to a lean worker costs fewer tokens and lands a better doc than the
    same task done in the parent (G23; the orchestration bench, report only).
- **Steps:**
  1. **O1** (into S13; it shipped without the queue and idle-parent delivery, so S29 builds them (user, 2026-10-23:
     "The parent should know everything")): `wait` events `turn_end`, `needs_input`, `failed`, `interrupted`,
     `gone`, `limit`, `fresh_context`, `quiet` (20 min with no CLI event, naming what runs); a durable per-parent
     queue with cursors (`record/worker-events.jsonl`); idle-parent delivery (Q9). Files: `crates/cli`, a new
     `M/worker_events.rs` (replaces `peer_wait.rs`), provider taps. Done: 10 fault tests, none twice. Opus, M–L.
  2. **O2** (into S13): `workers --json`/`show --json` add branch, base, ahead, behind, dirty, `merge` (clean /
     conflicts / merged / behind, via `git merge-tree`) and "base checkout clean". Files: `crates/cli`,
     `crates/git/src/repo.rs` (reuse `merge_tree`, `is_merged`). Done: scratch-repo tests per value. Codex, S–M.
  3. **O3** (into S14): state words from events (Working, Needs you, Quiet 20m, Failed, Stopped, Limit until
     HH:MM, Fresh context) and git (Done · not merged, Merged); archive's ask lists unmerged workers. Files:
     `WorkersStrip.tsx`, `stripView.ts`, `WorkerList.tsx`. Done: app tests per state, timings live. Opus, S–M.
  4. **O4** (into S15): the skill: fan out or not, a five-line brief (target, change, do-not-touch, files,
     acceptance commands), wave, wait, integrate small and often, land on, lean research and adversarial
     review and consultation in proportion to the work (G23); ≤ 150 lines. A user copy at
     `~/.brigadier/brigade/SKILL.md` overrides it ("Reset to shipped" in Settings). Done: skill tests + S15's live
     run. Opus, S.
  5. **O5** (after S15): the fault battery, the landing bench and the lean-research comparison as a nightly
     report, in `tools/bench/orchestration/`. Done when the first report has all three numbers. Codex, M.
- **Not copied:** a worker protocol of done, heartbeat and ask commands, report envelopes or decision gates.
  That is the push-and-gate pattern the strip-down removes. Permission prompts are never answered with key
  presses. Sessions never get `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS`.

### 9.2 Model advice and accounts

- **Best rival today:** bb switches accounts inside a turn, with no CLI restart. T3 Code catches two Claude limit
  forms we miss. No rival gives the agent one view of models, what's left and what fits
  (`EXCEL-model-accounts.md` §2).
- **Our bar:**
  - **No lost work.** A replayed mid-turn limit keeps the CLI session id, re-runs 0 tool calls, and the turn
    ends completed with no user action.
  - **Dead time.** Limit event → first event on the new account: p50 ≤ 10 s, p95 ≤ 30 s (cache-write tokens
    per switch reported). **Fresh:** every number in `brigadier models` is ≤ 60 s old; the call takes ≤ 200 ms.
  - **Honest forecast.** Median error of "runs out at" on 5-hour windows ≤ 20 min, else "estimate rough".
- **Steps:**
  1. **MA1** (any time): detect `api_error_params.rate_limit_info`, `result` with `api_error_status: 429`, and
     the gateway 503 "(reset after …)" text. Files: `P/claude/parse.rs`, fixtures. Done: each gives
     `ErrorKind::UsageLimit` with the right `resets_at_ms`; old limit tests pass. Codex, S.
  2. **MA2** (after S9): a switch line in `record/turns.jsonl` (from, to, kind, dead time, first call's cache
     writes) and a "Usage limit on A · continued on B" divider. Files: `M/conversation.rs`, `routing/meter.rs`.
     Done: a replay flow test keeps the session id, repeats no tool-use id and completes the turn. Opus, M.
  3. **MA3** (into S13b): `models` adds `you`, pooled windows, runway, each CLI's reported models and efforts,
     `fit` (ok / tight / out until HH:MM), `age_s`; text ≤ 25 lines; `forecast.rs`, `quota.rs` survive §3.6.
     Done: schema, `fit` and timing tests; a live check matches Settings › Usage. Codex, M.
  4. **MA4** (into S13): `brigadier start` answers with a receipt (requested vs effective model, effort,
     account); `wait` adds `usage_limit` and `account_switched`. Done when a worker whose provider is out raises
     `usage_limit` with `resets_at` and the other provider's fit (flow test). Codex, S.
  5. **MA5** (into S14, S23; needs Q11): "Resumes at" continues the turn at the reset (random spread, ≤ 4 tries,
     never for credit or spend limits; Continue now / Don't resume), replacing `wait_for_model` and the stand-in.
     Done: continues once; Don't resume stays idle; a credit limit never schedules (app + flow tests). Opus, M.
  6. **MA6** (after S14): amber account pill when `tight`, pooled bar in Settings › Usage, model · effort ·
     account on worker cards; A/B pictures first. Done: app tests and the screenshot pair. Opus, S–M.
  7. **MA7** (last): the rival bench and the forecast backtest as nightly reports, in `tools/bench/`. Done when
     the first report is filed. Opus, M.
- **Not copied:** no login proxy for the user's subscription traffic, no hidden CLI variables, no undocumented
  web usage endpoints (usage only via the CLIs' own channels; never a credential), no "unlimited" claims.

### 9.3 Browser and previews

- **Best rival today:** Claude Code Desktop finds and starts dev servers itself and verifies UI changes by
  default. Orca has the fullest scriptable browser: console, network, device emulation and trusted input
  (`EXCEL-browser.md` §2).
- **Our bar:**
  - **Servers.** Every server a session or its workers start is listed ≤ 1 s after it listens, however it was
    started. Attribution is 100% correct, and two sessions running `pnpm dev` at once don't collide.
  - **Check.** One `brigadier browser check` reports console errors, exceptions and failed requests, with a
    snapshot and a screenshot path: p50 < 1.5 s, ≤ 1.5k tokens. **Act:** 100% on a 20-case interaction bench.
  - **Zero setup.** A fresh user and project run a dev server and `browser check` with no prompt, dialog,
    download or file in the repo.
- **Steps** (all after S13):
  1. **BR1:** server discovery from the session's and its workers' process trees, `brigadier servers`, and
     `BRIGADIER_PORT` (base of 10 free ports). Files: a new `crates/core/src/servers.rs`, `M/preview.rs`,
     `crates/cli`, both provider launchers. Done: a fake CLI tree's background `http.server` (a worker's too) is
     listed and attributed within 1 s; two sessions get disjoint bases. Opus, M.
  2. **BR2:** a document-start script buffers console, errors, rejections and request outcomes;
     `browser console|network`. Files: a new `src-tauri/src/browser_watch.js`, `browser.rs`. Done: the fixture's
     error, 500, failed image and rejection reported in order, no duplicates after reload. Opus, M.
  3. **BR3:** `brigadier browser check`, plus a help line and a skill line: "after a UI change, run it before
     saying it works". Done: BR2's errors, snapshot and PNG path in one call, ≤ 1.5k tokens, p50 < 1.5 s. Opus, S.
  4. **BR4:** the full verb set: wait, hover, select, press, scroll, drag, upload, dialog, viewport, appearance,
     full and element screenshots, eval (agent tabs only), record. Done when every verb has a schema test, the
     viewport changes `innerWidth`, and dark flips `prefers-color-scheme`. Opus, M.
  5. **BR5:** a trusted-input spike, then the build: native events to the webview's window, never fronting it.
     Done when the 20-case bench is at 100% with no focus steal. Opus, Codex review, M.
  6. **BR6:** a Servers menu in the Browser (open, restart, stop) and "Point at it": a picked element goes to
     the composer as an attachment (HTML, styles, crop, source file:line). A .pen board comes first. Done live:
     the menu lists a CLI-started server, and a pick lands in the composer. Opus, M.
  7. **BR7:** the bench and the rival runs, in `bench/browser/`, nightly. Done when our numbers and the rivals'
     are recorded. Codex + Opus, S.
  8. **BR8** (later): `brigadier sim` over `xcrun simctl`, with taps through computer use. Done when a sample
     app boots, installs, launches, screenshots and takes a tap from the thread. Opus, M.
- **Not copied:** no config file in the repo for servers or ports. No third-party browser binary, vendored or
  downloaded. No driving of Claude in Chrome's bridge. Verification is a help line and a skill line, never an
  injected "please verify" (G1). Our names are "check", "Servers" and "Point at it".

### 9.4 Skills and MCP

- **Best rival today:** Superset: one install puts an MCP and its skills into both CLIs, in app and terminal,
  but only its curated plugins and only with an account (`EXCEL-skills-mcp.md` §2). Two holes of our own: extra
  Claude accounts may not see the user's MCPs, and `codex mcp add` in a session doesn't stick (X1, X6).
- **The mechanism:** the Library, `~/.brigadier/library/`, is a local plugin marketplace. Brigadier installs it
  through each CLI's own `plugin` commands, so it is the CLIs' native config in the app and in a terminal (G17).
- **Our bar:**
  - **One action.** From paste to working in Claude *and* Codex: 1 click, plus one browser consent per CLI for
    OAuth servers.
  - **Everywhere.** The item shows in a Claude thread, a Codex thread, a worker, plain `claude` and `codex` in
    a terminal, and a session on every extra account.
  - **Native only.** Brigadier writes no byte of `~/.claude.json` or `config.toml` itself, and puts nothing in
    `~/.claude/skills` or `~/.agents/skills`.
  - **No drift.** After one "Use in both" pass, every item on this Mac is in both CLIs, with no duplicate copies.
    **Next session:** an added item is there at the next session start, with no app restart.
- **Steps:**
  1. **X1** (any time): plugin-folder checks on the installed CLIs: `.mcp.json` variables on Codex, its
     command-path rule, name clashes, OAuth for plugin servers, project scope, the two holes live. Done:
     `research/SKILLS-MCP-PROBES.md` lists the commands run and what was seen. Codex, S.
  2. **X2:** the Library and packer, in a new `crates/registry/src/library.rs`: item → plugin, content-hash
     versions, sync through the CLIs' plugin commands. Done when a stub-home test adds a skill and an MCP and
     both CLIs list them. Codex, M.
  3. **X3:** the Add parser: both `mcp add` lines, `mcpServers` JSON, URL, folder, Git, skills.sh id. Done when
     a table test per format passes. Codex, S.
  4. **X4:** the Skills & MCP page: one list, a mark per CLI, one action per row, Add (power fields under
     Advanced); .pen design first. Files: `D/app/settings/integrations/*`. Done: a render test of the fixture
     setup shows the drift rows with "Use in both". Opus, M.
  5. **X5:** each CLI's own `mcp login` in sequence; `brigadier secret headers` and `brigadier mcp run` from the
     Keychain; `M/mcp_auth` and the worker-only translation go once X2 ships. Done: a local OAuth test server
     signs in from one click in both stub CLIs. Opus, M.
  6. **X6:** account and home parity (`P/accounts.rs`, `P/codex/home.rs`). Done: an account session gets the main
     one's init `mcp_servers`; a `codex mcp add` in a session survives the next launch. Opus, M.
  7. **X7:** `brigadier library list|add|remove|login --json`, on S13's CLI. Done: schema tests. Codex, S.
  8. **X8:** Move to Library with Undo (the original goes only after both CLIs list the new one), and "Update
     available" for items from Git or skills.sh. Done: move, undo and update tests on stub homes. Opus, S–M.
  9. **X9:** the bench in throwaway homes (stdio MCP, OAuth HTTP MCP, multi-file skill) and a click-count sheet
     for the rivals. Done: the first report. Codex, S.
  - Order: X1; then X2 ∥ X3; X5 ∥ X6; X4 ∥ X8; X7; X9 last.
- **Not copied:** no hosted proxy and no Brigadier account or cloud. No editing of the CLIs' config text. Never
  touch claude.ai-synced skills, Codex's own system skills or the vendor marketplaces on the machine.

### 9.5 Long sessions and token economy

- **Best rival today:** none runs with compaction off. T3 Code has the best hand-off: it carries the original
  request, the latest exchange and a token-budgeted verbatim slice, and works across providers
  (`EXCEL-long-sessions.md` §2).
- **Our bar:**
  - **Compaction in place** (behind `BRIGADIER_LONG_SESSIONS`, off by default, until the test round passes,
    run2-D15): 0 size seams on Claude and Codex; the CLI compacts in the same session (run2-D2, run2-D5,
    run2-D6). After every compaction, every active ledger entry arrives whole, exactly once, by the receipt
    (run2-D8). A fresh session only on the fallbacks (run2-D10); "above half the point after compacting" is
    proposed, waiting on the test round. With the switch off, the old bar holds: **no compaction**, 0 in
    Claude sessions of any length (Codex's are counted, no bar).
  - **Fewer tokens:** price-weighted ≤ 0.95× the plain CLI on the long battery (past 400k), ≤ 1.01× on the
    short one (< 150k).
  - **Recall** ≥ 90% of planted items across compactions (switch on) or forced seams (switch off). The **cold
    return** bar went with S24; the nightly battery reports cold-return cost, never gates (Delegator's
    judgement, 2026-10-23: the research's saving is one uncached read per cold return; plain Claude Code never
    restarts on a cold cache; S24 stays dropped; the nightly battery reports cold-return cost, never gates).
    **Done:** each scripted task still passes its test command.
  - **Decisions survive** (CONTINUITY §3.7) (user, 2026-10-23: "find a fix for this"): after three compactions
    (switch on) or three seams (switch off), ≥ 9 of 10 planted items applied without being asked where they
    apply; a brief derived from them has 0 contradictions and 0 additions not marked "proposed"; every open
    item is raised again. The plain CLI with its own compaction is the comparison.
- **Steps** (they extend S17–S27 and add no stage):
  1. **L1** (into S18): `brigadier transcript search|read` over the lineage's CLI files, `tool-results/` and
     rollouts included, no database copy. Files: `crates/brain/src/transcript*`, `crates/cli`. Done: fixture
     tests of both formats; a live read returns a persisted large output verbatim. Codex, M.
  2. **L2** (into S22): bootstrap order: map, ledger, note, the session's first user message and the latest
     exchange verbatim, a newest-first tail of ≈15k tokens, one line per omitted range with its exact read
     command. Done: a test asserts the order and that every omitted range's command works. Opus, S.
  3. **L3** dropped with S24: a cold cache continues the same session (S24's decision). An account switch or a
     model change also continues the same session; none starts a successor
     (user, 2026-10-23: "sure, continue the same session, don't start a fresh one").
  4. **L4** (into S21, S25): a token A/B arm, the same scripts in the plain CLI and Brigadier, both providers.
     Done: the nightly artifact shows all five bars per provider; the first run is the baseline. Codex, M.
  5. **L5** (into S25, no weekly scan existed at S23): a compaction counter in the weekly scan of real sessions. Done when the report lists
     any compaction with its session id. Codex, S.
  6. **L6** (into S26): the Codex 1M-window probe, and the divider on Codex's compacted events. Done when the
     probe is written up and a recorded rollout shows the divider. Codex, S.
  7. **L7** (S27, unchanged): the A/B of `bashOutputMaxChars` and the exact-read instructions. Opus, S.
  8. **L8** (= S22b) (user, 2026-10-23: "find a fix for this"): entry ids and the time from `brigadier ledger add`,
     `ledger close|show --open|check`, the bootstrap that carries open items, rules and decisions whole, the seam's
     check and take-over lines, four `/brigade` lines, and the battery's decision-survival block. Done: S22b's
     done-when. Opus, S–M.
  9. **L9** (into S25) (user, 2026-10-23: "find a fix for this"): the decision-survival block nightly on both
     providers, next to a plain-CLI arm that compacts. Done when the nightly artifact shows the "decisions survive"
     bar per provider. Codex, S.
- **Not copied:** no rival's words for compaction or hand-off in the UI. Before the first nightly numbers, no
  "fewer tokens" claim on the landing page.

### 9.6 Open decisions and what waits on the user

**Decided (10-22):**
- **Q9. Notify an idle parent when a worker finishes?** Yes. With the parent's turn ended and no `wait` running,
  one batched message: "Worker *title*: finished · *excerpt*", a result of what the thread started (G1), never
  twice. (O1) Not built in S13; S29 builds it, for questions, prompts and stops too (user, 2026-10-23: "The parent
  should know everything").
- **Q10. A warm note before the cache expires?** No. S24 stays dropped; cold-return cost is reported, never
  gates (Delegator's judgement, 2026-10-23: the research's saving is one uncached read per cold return;
  plain Claude Code never restarts on a cold cache; S24 stays dropped; the nightly battery reports
  cold-return cost, never gates).
- **Q11. Resume visibly at the reset when every account is out?** Yes (MA5). The "Resumes at" card, then the
  session continues visibly at the reset — a divider + the same turn resumed — like the plain CLI. This
  replaces "a usage limit stops the turn … never restarts quietly" everywhere.
- **Q12. The "carry on" note after an account switch?** Yes: the one visible "carry on" line after an account
  switch stays. It is engine-written text inside the user's own turn, like the CLI's own auto-continue. (MA2)
- **Q13. Ship `/brigade` into Codex through the shared plugin folder?** Yes: it ships into Codex through the
  shared plugin folder; it may appear in plain-terminal Codex sessions and says it needs the app. (X2; §8,
  item 7)

**Decided (10-23): the 20 drifts of the w95 audit** (run `20261022-0805-take-over-5-sessions`,
`workers/w95/report.md`). Item numbers are the audit's.
1. (D1, D20) Chats can't run workers → a Chat is always a brigade and acts almost exactly as a normal session (§1 Teams,
   S15, S28) (user: "a chat is always a brigade").
2. (D2) Workers can't start workers → stays: they use their CLI's own subagents (§1, S13, S15, §8 item 13) (user:
   "workers cant start other workers").
3. (D3) Unclear asks → when the user is uncertain, a model fusion gives the best answer (S15, S36) (user: "if the
   user is uncertain - we help him").
4. (D4) The context ring in Chats → stays as it is (§1 Context card) (user: "ok").
5. (D5) Computer use reaches no session → fixed (§1, S13, S35; #529) (user: "fix this!").
6. (D6) PR row words and colours → the grill's list (§1, S14, S34) (user: "fix this").
7. (D15) Codex network at Ask and Accept edits → stays on so `brigadier` reaches its socket (§8 item 5;
   [#538](https://github.com/syncra-studio/brigadier-ai/issues/538))
   (Delegator's judgement with the user's trust, 2026-10-23: "do as u thinnk it's best, i'll trust your judgement").
8. (D7) An idle parent isn't told → the parent knows everything about its workers (§1, S13, O1, Q9, S29) (user:
   "The parent should know everything").
9. (D8) The successor waits → it starts at once from the note (§1, S22, S23) (user: "Why wait? See how delegator
   works").
10. (D14) A successor on account switch or model change → the same session continues (S23, L3)
    (user, 2026-10-23: "sure, continue the same session, don't start a fresh one").
11. (D16) The cold-cache rule → dropped (S24, L3, the cold-return bar)
    (Delegator's judgement, 2026-10-23: the research's saving is one uncached read per cold return; plain
    Claude Code never restarts on a cold cache; S24 stays dropped; the nightly battery reports cold-return
    cost, never gates).
12. (D9) No session-to-session messages, "read anything" in one project → sessions message each other when asked
    or truly needed, and read every project (§1, §2, S13, S30) (user: "Sessions should be able to message
    eachother").
13. (D21) `tmp/` emptied each turn → the grill's design stands: a plain `TMPDIR`, cleaned only on Archive and Delete
    (§1 Folders, S31; #532).
14. (D10, D22) No Settings page for routing → Settings › Routing comes back as the rules editor, and `brigadier start` takes
    model and effort from those rules (§1, S13b, S32) (user: "user should've been able to apply his rules").
15. (D17) Brigadier overrides `disableAllHooks` → removed (S37, §1, §8 item 17)
    (Delegator's judgement, 2026-10-23: parity wins; Brigadier stops overriding disableAllHooks).
16. (D11) The shipped rules file carries the user's personal rules → a neutral shipped default; those rules live in
    the user's own file (§1, S13b, S33).
17. (D12) Only one session may use the Local checkout → stays as built.
18. (D12) The CLI's own question tool is denied → stays as built (S17).
19. (D12) Folder trust forces Ask and stops workers → stays as built.
20. (D1) A Chat's shell can run only `brigadier` → folds into 1: the CLI's own shell, cwd = the Chat's session folder
    (S16c, S28).
Also decided on 10-23, with no step of their own: D13 (Sources are outside-the-repo things only, and the
ledger is not shown in the UI, `f10bb20b9`), D18 (the Delegator skill's continuity items) and D19 (the memory
cleanup): no step needed. D23 is S38; D51 (10-24) is S39; D24 is S23's kept `orch:<id>` log (§4 last consumers, #540).

**Waiting on you:**
- A terms check of multi-account auto-switch before marketing it.
- Bench sign-ins for Superset and Orca (the orchestration and model-advice benches need them).
