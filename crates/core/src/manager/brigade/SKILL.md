---
name: brigade
description: Run a job with workers of your own when it calls for them - parallel parts, a long build, research, a second vendor's review - through the `brigadier` command. Needs the Brigadier app. Read it before your first `brigadier start`.
---

# Brigade

You are a session in the Brigadier app. Most jobs you do yourself, as any coding session would. When a job calls for it, you can run **workers**: plain Claude Code or Codex sessions that you start, brief, wait on and land. They show only inside your session (its Workers row and rail), never in the sidebar. The user talks only to you; you relay.

Everything goes through `brigadier` (`brigadier <command> --help` for details; `--json` on every command):

| Command | Use |
|---|---|
| `brigadier models` | Before each start: which provider, model and effort fits, what's left on each account, the user's rules. |
| `brigadier start "<brief>" --title T [--work KIND] [--agent claude\|codex] [--model M] [--effort E] [--base B] [--local] [--repo PATH] [--permission P] [--lean]` | Start a worker. It gets its own worktree on a branch from your current branch (`--base` to change) and your permission level. Prints its id. |
| `brigadier wait [<id>…] [--timeout S]` | Block until a worker event: turn ended (with its last message), needs input, gone, usage limit, fresh context. Workers send you nothing on their own: this is how you hear from them. |
| `brigadier read <id> [--last N]` | Its last message, or its last N items. |
| `brigadier send <id> "<text>"` / `--allow` / `--deny` | Message a worker (queued mid-turn), or answer its permission prompt. |
| `brigadier workers` / `brigadier close <id>` | List yours; close one when its work is in. |
| `brigadier ask "<question>" [--option O]…` | Ask the user on a card. Recommended option first. |

In a Chat, a worker works in the repository `--repo <path>` names (a worktree of it, or its checkout with `--local`), or without `--repo` in a folder of its own with no git. A worker can't start workers of its own.

## Fan out or not

- Default: do it yourself. A worker costs a fresh context and a merge.
- Workers pay for: parts that can truly run at once on **disjoint files**; a very long job (hours, or more than your context should carry); a second vendor's view; research you want kept out of your context.
- Small reads go to your CLI's own subagents first. Bigger research goes to a **lean** worker (`--lean`: just the goal and the repo, none of the user's MCP servers or skills, no memory writes). It reports conclusions with file:line.
- Unclear ask: ask the user **once** (`brigadier ask`). If the options need evidence, one lean researcher. Build competing prototypes only when the user must compare them, never by default.
- Overnight or "until 07:00" is the same skill with a deadline: plan the waves to end 20 minutes early and leave a report.

## Pick the model

Read `brigadier models` before each `brigadier start`. Pick the row whose Work fits the job and pass it as `--work "<Work>"`: the row (the user's, else the shipped one) picks the agent, model and effort, and the receipt names it. Add `--agent`, `--model` or `--effort` only to override one of them, from the guidance and what is left. The user's rules shown there bind you; `start` refuses what they forbid. A provider out of quota: override with the other one for bounded work.

## Brief: five lines

Every brief has these, in plain words:

1. **Target:** the repo area and the outcome.
2. **Change:** what to build or fix, and the decisions already made (quote the user's words when they matter).
3. **Do not touch:** files, areas or behaviour that must stay.
4. **Files:** the exact files or names it owns. Two workers never share a file.
5. **Acceptance:** the exact commands that must pass; "commit, merge into my branch (below), then say done with the commit ids".

Add "Ask me (your parent) if something blocks you; don't guess on scope." The worker sees no Brigadier protocol; it is a plain session.

## Waves

1. Start the wave (≤ 3 workers unless the user said more).
2. Wait right after starting them; never end your turn with workers running and no wait on them. In Claude Code run `brigadier wait` as a background command and end your turn: you are woken when it returns. In Codex run it in the foreground. Each event returns the wait; handle it, then wait again until every worker's work is in:
   - **turn ended:** read the last message. If done: check its claims yourself (`git log`, run its acceptance commands). If it asks something: answer it if the plan and the user's words settle it, else ask the user once and relay with `brigadier send`.
   - **needs input** (a permission prompt): allow it with `brigadier send <id> --allow` if it is inside the brief and harmless. Otherwise ask the user, or deny and send a note.
   - **usage limit:** let it wait for the reset, or close it and restart its rest on the other provider.
   - **fresh context:** its CLI compacted; check that its next message still knows the brief, or resend the brief's five lines.
   - **gone:** read its branch, then start a successor with "continue from branch X; `git log` first".
3. A worker that is quiet for 20 minutes with nothing running: `brigadier read <id> --last 10`, then nudge it or close it.
4. Next wave when this one is integrated.

## Integrate small and often

- A worker merges each finished step into your branch itself, not one big merge at the end. Your branch is checked out in your workspace, so the brief says where, with the real path and names filled in: "When your checks pass, merge your branch into mine: `git -C <my workspace> merge --no-edit <your branch>` (if mine moved and it conflicts, rebase on it first). Report the merge commit." Don't edit your workspace while a worker may be merging into it.
- You decide merge or rebase and resolve conflicts. Ask the worker for help when it knows the change better.
- In a Local checkout, commit your own edits before you integrate a worker.
- After integrating, run the checks on the integrated tree yourself. Then land on what the user chose: the New worktree's base branch, or the Local checkout's branch. A PR instead is fine when the user prefers it.
- Close each worker once its work is in.

## Reviews and consultation

- Adversarial review: `codex review` (or the other vendor's CLI) once per step on the integrated tree, never mid-run. Fix what you agree with. One review, one fix pass, then report what is left; never loop.
- Before you approve a plan or outline, or take an irreversible step (a delete, a merge into the user's branch, a publish), get a critique from the other vendor's CLI. Take what you agree with. Not for every small decision.

## Gotchas

- Worktrees don't remove collisions; they move them to merge time. Two workers that touch a shared file (a registry, a type, a string catalogue) run in order, or get their exact names fixed up front.
- Tests that need gitignored files (fixtures, local databases, credentials) can skip silently in a fresh worktree. The worker copies or links them in and says which tests skipped.
- A worker's claim is not a fact until you checked it. Say whose it is when you report.
- Your context fills too: past the seam you'll be asked for a hand-off. Write it with each worker's id, branch and state, and what you were waiting on.

## Report to the user

When the job is done: what landed where (commits, branch), what each check showed, what a review found and what you fixed, and what only the user can do. Keep it short.
