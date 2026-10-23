# Brigadier

[![CI](https://github.com/syncra-studio/brigadier-ai/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/syncra-studio/brigadier-ai/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Platform: macOS](https://img.shields.io/badge/platform-macOS%2014%2B-lightgrey.svg)](#status-and-roadmap)

**Brigadier runs your CLIs — Claude Code and Codex today, more soon — as one team on your Mac.**
You talk to one thread: a plain Claude Code or Codex session. It does most jobs itself, and when
a job calls for it, starts other CLI sessions as workers in their own git worktrees, getting a
review from the other vendor before it merges their work.

- **Workers, cross-vendor review.** When a job calls for it — parallel parts, a long build,
  research, a second opinion — the thread starts a worker: a plain Claude Code or Codex session
  of its own choosing, in its own worktree. A worker runs its own checks, gets a review from the
  other vendor's CLI (`codex review`, or Claude on Codex's work), and merges into the thread that
  started it.
- **Computer use in the background (macOS).** Workers can open and use the apps they build, and
  any other app a task needs, without moving your cursor or bringing the app to the front: a
  step that would interrupt you is refused. System Settings is read-only to them. The
  helper builds on a vendored open-source driver:
  [helpers/computer-use/README.md](helpers/computer-use/README.md).
- **Long sessions without compaction.** The thread never compacts its context. Before it fills, a
  fresh session takes over from a handoff note, every decision so far and the latest messages.

Free and open source (MIT). Brigadier runs the `claude` and `codex` CLIs you already have, under
your own logins. It holds no API keys and never reads your credentials.

## Install

macOS 14 or later, Apple silicon or Intel. Signed and notarized.

```sh
brew install --cask syncra-studio/tap/brigadier
```

Or download `Brigadier-0.1.0-alpha-universal.dmg` from the
[latest release](https://github.com/syncra-studio/brigadier-ai/releases/latest) and drag
Brigadier to Applications. It updates itself. More ways, and how to uninstall:
[docs/user/install.md](docs/user/install.md).

You also need at least one of [Claude Code](https://docs.anthropic.com/en/docs/claude-code) and
[Codex](https://github.com/openai/codex); Brigadier's setup can install them. Both is
better: each reviews the other's work. To build from source (or for Windows and Linux), see
[docs/DEVELOPMENT.md](docs/DEVELOPMENT.md). Windows builds a per-user installer
(`…-x64-setup.exe`) there; Windows releases aren't published yet
([docs/user/install.md](docs/user/install.md#windows)).

## Quick start

1. Open Brigadier. Its setup connects your agents: it finds `claude` and `codex`, and can
   install them or sign them in for you.
2. Pick a project: one it suggests from your recent Claude Code and Codex work, or any git
   repository (**+** next to **Projects**). It can also clone one, or start git in a folder.
3. Press **New session** on the project and say what you want in plain words.
4. The session works on its own branch, in its own worktree: it plans and changes the code,
   running your checks and getting the other vendor's review itself. Then it merges the work, and
   a **Revert** button sits under the merge message.

Step by step: [docs/user/first-task.md](docs/user/first-task.md).

## Sessions

One session is one task: a chat with an agent, on its own branch and worktree. The sidebar lists
each project's sessions with what each is doing and why (Working · waiting for reset 14:30,
Needs you · question, Ready · PR open, Merged); the ones that need you come first. For a second task at the
same time, start another session: **New session** on the project, or **⌘Enter** in the composer
to start it in the background. A session can also offer to start something outside its task as
its own session. The tab bar holds the open session's chat and its own terminals, browser pages
and files. More: [docs/user/sessions-and-tabs.md](docs/user/sessions-and-tabs.md).

## Auto-merge and Revert

Finished work merges on its own: through a pull request when the project is on GitHub and `gh` is
signed in, otherwise into the base branch on your Mac. Checks and review inform the merge without
holding it; the merge message says what was verified. Each merge message has a **Revert** button.
Turn it off in Settings › Git. More: [docs/user/auto-merge-and-revert.md](docs/user/auto-merge-and-revert.md).

## Accounts

Brigadier uses the logins your CLIs already have; there is no Brigadier account. Add more Claude
Code or Codex accounts in Settings › Providers; when one runs out, work moves to another.
More: [docs/user/accounts.md](docs/user/accounts.md).

## Privacy

- Runs your own `claude` and `codex`, unmodified, under your own logins.
- Never reads your login tokens or API keys, and holds none.
- No account, no server, no telemetry. Your sessions and settings stay in
  `~/Library/Application Support/Brigadier`.

What it does fetch (updates, the model list, two small local models):
[docs/user/privacy.md](docs/user/privacy.md).

## Status and roadmap

Brigadier is alpha software, built in the open by one developer since September 2026. Expect
rough edges.

- **macOS** is the main platform, and every feature here works on it.
- **Windows and Linux** build and pass a launch check in CI (Linux on every push, Windows
  nightly), but are not ready for real use. Computer use and browser data import are
  macOS-only for now. Agents in the browser work on macOS and Windows; Linux has no
  built-in browser.
- **Agents:** Claude Code and Codex today. Next: the Cursor, opencode and Grok CLIs, and local
  models.

## What's in it

- **Sessions.** One thread per task, on its own branch and worktree. It reads and runs things,
  makes small changes itself, starts workers for the rest, and merges the result (with a Revert
  button).
- **Overnight runs.** Ask a session to work until a time ("until 07:00"). It plans its workers
  to finish before the deadline and leaves a report.
- **Long commands in the background.** A command the thread runs that is still going after a
  minute moves to the background with a live line, and its result comes back to the thread when
  it ends. A turn's steps fold under "Worked for …" once the answer starts. A sidebar dot marks a
  conversation whose request finished since you last opened it.
- **Chats and side chats.** A Chat is a plain conversation with one model and web search. A side
  chat asks a question about a session without adding it to the session's thread.
- **Several accounts per agent.** Sign in to more than one Claude Code or Codex account and pick
  which one new conversations use (Settings › Providers). When one runs out, Brigadier switches
  to another (a setting, on by default).
- **Usage and fallback.** The sidebar's usage button and Settings › Providers show each
  agent's usage windows and where they are heading. When an account runs out, Brigadier switches
  the conversation to another account of the same agent automatically (a setting, on by default);
  only once every account of that agent is used up does work move to the other agent, and it goes
  back after the reset.
- **Built-in browser** (macOS, Windows). Opens the pages agents and you work on, with history and
  downloads. On macOS it also has per-site camera and microphone choices, and can import cookies
  and history from your other browsers. On macOS agents use it too: they read and work a page in a
  background tab with your sign-ins, you take over by using the page (**Continue** hands it back),
  and paying, signing in or sending in your name asks you first.
- **Terminals, files, review and source.** Terminal tabs, a file tree, a review of everything a
  session changed, and git actions beside the thread.
- **Dictation and read aloud.** Dictation runs a speech model on your computer, so the audio never
  leaves it. Read aloud uses the system voice.
- **Clean up after itself.** Archiving a session removes its worktrees, scratch folders, CLI
  session files and processes. **Uninstall Brigadier…** (in the menu bar and Settings › Storage)
  removes what it made.

## How it uses your CLIs and accounts

Brigadier drives the `claude` and `codex` CLIs installed on your machine, found on your login
shell's `PATH`, with the logins you already have. It never reads their credentials and holds no
API keys of its own. Each extra account you add gets its own CLI home inside Brigadier's data
folder, so your terminal login never changes. Brigadier changes the CLIs' own config only to undo
a trust entry a session made Codex write. The files a session makes a CLI write (Claude's
transcript and task files, Codex's rollout) are removed when the session is closed or archived.

Each project can name gitignored env files (such as `.env.local`) as secrets: they are copied into
each worker's worktree, never committed, and their values are redacted from everything Brigadier
records.

## How it works

### Sessions

A session is one long-lived CLI session (Claude Code or Codex), the **thread**, that talks with
you. It reads code, runs builds and tests, and makes most changes itself. For a part that calls
for it — disjoint parallel work, a long build, research, a second vendor's view — it starts a
**worker**: a plain Claude Code or Codex session of its own choosing, through the `brigade` skill
and the `brigadier` command. A worker gets its own worktree on a branch from the thread's current
branch, and the thread's permission level; it follows your repository's `CLAUDE.md` and
`AGENTS.md` like any other session. It shows only inside the session that started it (its Workers
row and the rail above the composer), never in the sidebar, and it's read-only to you: you answer
its questions in the parent thread, which relays them with `brigadier send`.

A worker runs its own checks and reports when it's done. Its commits then **land**:

- **Merged into its parent**, with plain git. The thread decides merge or rebase, and resolves
  conflicts with the worker's help.
- **Checked again** on the integrated tree, by the thread itself.
- **Reviewed.** Before it merges, `codex review` (or the other vendor's CLI) looks at the change
  once, on the integrated tree, never mid-run. The thread fixes what it agrees with, then reports
  what's left.

When the thread's own work is ready, it merges it the same way: through a pull request when the
project is on GitHub and `gh` is signed in, otherwise on your Mac. The merge message has a Revert
button that asks the thread to undo it the way it went in. With Settings › Git › "Merge on its own
when the work is ready" off, the thread tells you the work is ready and merges when you ask.

Archiving a session removes its worktrees, scratch folders, CLI session files and processes,
including detached children. Unfinished work is kept as a WIP commit on its branch. Anything that
couldn't be removed is retried at the next launch.

### Permission levels

A session's permission level is picked in the composer. New sessions start in **Accept edits**;
Settings › Configuration changes the default. The level goes straight to the CLI as its own
setting: Brigadier adds no sandbox, no reviewer and no permission rules of its own, and the CLI's
own permission prompts reach you as cards.

| Level | Claude Code | Codex |
|---|---|---|
| **Ask** | `default` mode: asks before edits and commands | `untrusted` approvals, `workspace-write` sandbox |
| **Accept edits** | `acceptEdits` mode: edits files freely, asks before other commands | `on-request` approvals, `workspace-write` sandbox |
| **Full access** | `bypassPermissions`: never asks | `never` approvals, `danger-full-access` |

Codex's sandbox is Codex's own setting, so it stays at Ask and Accept edits; Brigadier keeps
Codex's network on at Ask and Accept edits because the `brigadier` command needs the daemon
socket (Full access is Codex's own). At Full access a session can read, change and delete any
file your account can, Brigadier's own data included, run any command and use the internet,
without asking.
The composer shows a notice while a conversation is in Full access. A folder you don't trust runs
every session there at Ask.

At every level, workers and the thread push, publish, deploy or open pull requests when the
task calls for it, with your own git and `gh` sign-ins. The thread asks for your approval before
it spends money, uses credentials or the keychain, or destroys something outside the session's
own work; a worker lists such steps for you instead.

### What still depends on trust

- Workers and threads get a session token, checked on every call to `brigadier` and revoked when
  the session ends. Requests only the UI may make (such as
  answering approvals) need the IPC token in `<data>/run/`. Sessions run as their CLI would on
  its own, so a hostile one could read the token. On macOS a process inside Codex's sandbox still
  can't act as the UI: the daemon refuses the token from any sandboxed process. A Claude Code
  session, or a Codex one at Full access, has no sandbox, and nothing refuses it.
- A process that detaches and moves out of Brigadier's folders escapes the cleanup.
- Codex writes a trust entry into `~/.codex/config.toml` when a thread starts with a writable
  sandbox in a folder you never trusted. Brigadier starts every thread without a sandbox of its
  own and sets it per turn, so Codex writes none. If one ever appears for a folder Brigadier
  created, it is removed when the session closes, and only while it is still exactly `trusted`.

To report a security problem, see [SECURITY.md](SECURITY.md).

### Project Brain

Each project has a Brain: what Brigadier knows about it, so a question answered once is not
investigated again. It is a graph of modules, file summaries, decisions, conventions, reports and
research, each with where it came from (the code index, a worker's report, the thread, you), and
from which session, model and commit. It also keeps what you settle. Reports, plan decisions and
your answers on cards are recorded without being asked.

- **Records search.** `brigadier brain search` searches what the project's sessions already keep:
  their CLI transcripts across every fresh context (with the whole outputs the CLI saved), their
  workers', their ledgers and hand-off notes. `brigadier brain read` gives a transcript span
  verbatim. A ledger entry learned from reading points at its file and reads as stale once the
  file changed.

- **Code index.** The repository is scanned (tree-sitter symbols and references, manifests,
  scripts) and kept current by a file watcher. The thread and its workers search it
  (`brigadier code search`) before reading files.
- **Staleness.** Nodes remember the content hash of the files they describe. When a file
  changes, they're marked stale and answered as "may be outdated".
- **Spare quota.** After a project's first scan, the cheapest model maps its modules, stack,
  conventions and build recipe. Quota that would expire unused refreshes stale nodes and fills
  gaps, and it stops as soon as your own work starts.
- **Personal memory.** Stable facts about you (where you live, your work, your tools, what you
  prefer) are given to new threads and Chats; you see, edit and delete them in Settings › Memory.
  Passwords, keys, card and account numbers are never kept.

Everything stays in the data folder: `brains/<project>/brain.sqlite`, `index.sqlite`,
`brains/personal.sqlite`, and the embedding model under `models/embeddings/`.

### Models and quota

Brigadier picks no model for you: the thread reads `brigadier models` and picks provider, model
and effort itself, for every session it starts.

- **`brigadier models`.** Before it starts a worker, the thread reads this: the models each CLI
  reports as available, usage and reset times per account, the current account, a shipped
  guidance table (which provider, model and effort fits what kind of work, with a one-line reason
  each), and your own rules on top.
- **Your rules.** `~/.brigadier/models.md` is your own editable file: forbidden models, an effort
  ceiling, allowlists. The shipped default carries today's rules (never Fable, effort ≤ high).
  `brigadier start` refuses only what this file forbids, and quotes the rule.
- **Registry.** `registry/models.json` rates every model Brigadier drives: strengths per kind of
  work, efforts, context window. The app checks this repository for a newer revision daily. It
  takes one only if it stays within fixed bounds and has a higher revision.
- **Quota.** Brigadier reads Claude's and Codex's usage windows and projects each one to its
  reset. When an account runs out, Brigadier switches the conversation to another account of the
  same agent automatically (a setting, on by default).
- **Fewer tokens, built in.** Nothing to turn on. A thread idle past its prompt cache restarts
  from a checkpoint briefing instead of resending its whole history. A worker at about two-thirds of
  its model's context window (200k tokens at most) hands over to a fresh session of the same model.
  Brain answers return a few results of each kind and page the rest.

### Settings

Settings is grouped into Personal (General, Voice, Configuration, Memory, Keyboard shortcuts),
Agents (Providers, Plugins, Git, Computer use), Browser & Data (Browser, Import Browser Data) and
System (Storage, Inspector), with Archived chats at the end. Settings › Computer use (macOS) turns it
on or off and walks through its Accessibility and Screen Recording permissions. Accounts:
[docs/user/accounts.md](docs/user/accounts.md).

## Uninstall

Use **Uninstall Brigadier…**, from the menu bar (Brigadier ▸ Uninstall Brigadier…) or Settings ›
Storage. It stops every conversation as archiving does (unfinished work kept as WIP commits), then
removes everything Brigadier recorded making: worktrees, scratch folders, CLI session files and
processes. If the app is already deleted, run the script from this repository (its options are in
[docs/DEVELOPMENT.md](docs/DEVELOPMENT.md#the-uninstall-script)):

```sh
scripts/uninstall.sh --dry-run --app-id ai.brigadier.app
scripts/uninstall.sh --app-id ai.brigadier.app
```

On Windows, Uninstall Brigadier… is in Settings › Storage; then remove the app in Settings ›
Apps › Installed apps. The script there is `scripts\uninstall.ps1 -DryRun -AppId ai.brigadier.app`
(and without `-DryRun`), which also runs the app's own uninstaller.

Neither removes the computer-use helper's Accessibility and Screen Recording entries: remove
"Brigadier Computer Use" from System Settings › Privacy & Security yourself.

## Contributing, security, license

- [CONTRIBUTING.md](CONTRIBUTING.md): read it before you open an issue or a pull request. Pull
  requests need an accepted issue first, and they're closed automatically without one.
- [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md): building, checks, layout and dependency notes.
- [SECURITY.md](SECURITY.md): report vulnerabilities privately, never in a public issue.
- [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md): Contributor Covenant 2.1; report problems to
  hello@syncra.studio.
- [docs/](docs/README.md): design plans and dated evidence (measurements and acceptance runs).
- MIT, see [LICENSE](LICENSE). Third-party notices: [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

Brigadier is made by SYNCRA SRL. Founded and developed by Stephen Golban.
