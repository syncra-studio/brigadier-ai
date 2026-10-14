# Brigadier

[![CI](https://github.com/syncra-studio/brigadier-ai/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/syncra-studio/brigadier-ai/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Platform: macOS](https://img.shields.io/badge/platform-macOS%2014%2B-lightgrey.svg)](#status-and-roadmap)

<!-- Hero GIF goes here, above the pitch, once a real capture exists. -->

**Brigadier runs Claude Code and Codex as one team on your Mac.** You talk to one thread. It
plans the work, hands it to agents running in parallel in their own git worktrees, and the other
vendor's model reviews every change after it lands.

- **Parallel agents, cross-vendor review.** Each task runs in its own worktree on the model
  best suited to it, and its commits land by fast-forward. Every landing then gets a read-only
  review from the other vendor (Codex reviews Claude's work, Claude reviews Codex's) in the background.
  The findings go back to the thread, which fixes them or says why not.
- **Computer use in the background (macOS).** Workers can open and use the apps they build, and
  any other app a task needs, without moving your cursor or bringing the app to the front. In
  its benchmark, element actions hit their target 1,600 times out of 1,600, and reading a
  window's controls takes about 6 ms (median). Across three runs of its 29-task suite, Claude
  workers completed 85 of 87 runs. Method and numbers:
  [docs/COMPUTER-USE-PLAN.md](docs/COMPUTER-USE-PLAN.md).
- **Long sessions without compaction.** The thread never compacts its context. Before it fills, a
  fresh session takes over from a handoff note, every decision so far and the latest messages.
  Workers start with what the thread already read, search a local code index before reading
  files, and ask the Project Brain what earlier sessions learned.

Free and open source (MIT). Brigadier runs the `claude` and `codex` CLIs you already have, under
your own logins. It holds no API keys and never reads your credentials.

## Install

There is no prebuilt release yet. Build it from source on macOS 14 or later:

1. Install [Rust](https://rustup.rs) (the pinned toolchain installs itself), Node 24+, pnpm 12,
   CMake, and the Xcode Command Line Tools (`xcode-select --install`).
2. Build the app:

   ```sh
   git clone https://github.com/syncra-studio/brigadier-ai.git
   cd brigadier-ai
   pnpm install
   pnpm tauri build --bundles app
   ```

3. Move `target/release/bundle/macos/Brigadier.app` to `/Applications` and open it.

You also need at least one of [Claude Code](https://docs.anthropic.com/en/docs/claude-code) and
[Codex](https://github.com/openai/codex), installed and signed in. Having both is better: cross-vendor
review needs both. Windows and Linux builds: see [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md).

## Quick start

1. Open Brigadier. First-run setup finds `claude` and `codex` on your login shell's `PATH` and can
   start their install or sign-in for you.
2. **Add a project** in the sidebar: a folder with a git repository (Brigadier can create one for
   a folder that has none, or clone one).
3. Pick a permission level in the composer and say what you want, in plain words. New projects
   start in Full access, where workers run like your own terminal; read
   [Permission levels](#permission-levels) first.
4. Watch the thread plan it and start workers. Each worker's progress shows in the thread. Work
   lands on your branch, or on a session branch that merges into its base when you ask.

For a plain conversation with one model, start a **Chat** instead of a session.

## Status and roadmap

Brigadier is alpha software, built in the open by one developer since September 2026. Expect
rough edges.

- **macOS** is the main platform, and every feature here works on it.
- **Windows and Linux** build and pass a launch check in CI on every push, but are not ready for
  real use. The worker sandbox, computer use and browser data import are macOS-only for now,
  and Linux has no built-in browser.
- **Agents:** Claude Code and Codex today. Next: the Cursor, opencode and Grok CLIs, and local
  models.

## What's in it

- **Sessions.** One thread per piece of work. It reads and runs things itself, makes tiny edits,
  and starts workers for the rest. It works on your checked-out branch, or on a new worktree and
  branch that merges into its base when you ask.
- **Overnight runs.** Type `/overnight` with a plan and when it should be ready. The session works
  through the plan phase by phase until it's done or the deadline comes, then leaves a morning
  report and a notification.
- **Chats and side chats.** A Chat is a plain conversation with one model and web search. A side
  chat asks a question about a session without adding it to the session's thread.
- **Several accounts per agent.** Sign in to more than one Claude Code or Codex account and pick
  which one new conversations use. When one runs out, Brigadier switches to another (a setting,
  on by default).
- **Usage and fallback.** The Usage page shows each agent's usage windows and where they are
  heading. A worker whose provider hits its limit hands its task to the best other model, in the
  same worktree. A conversation continues on the other vendor and goes back after the reset.
- **Built-in browser** (macOS, Windows). Opens the pages agents and you work on, with history and
  downloads. On macOS it also has per-site camera and microphone choices, and can import cookies
  and history from your other browsers.
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

A session is one long-lived CLI session (Claude or Codex), the **thread**, that talks with you.
It reads code, runs builds and tests, and makes tiny edits itself. Everything else goes to
**workers**: one CLI session per task, on a model the router picks for that kind of work. A
write task gets its own worktree on a task branch and a scratch folder outside the repository.
It starts with a context pack (what the thread already read and searched) and with the
dependency installs and build caches of your checkout copied in, so its first build doesn't
start from nothing. Workers follow your repository's `CLAUDE.md` and `AGENTS.md` whichever
vendor runs them.

A worker reports when it's done. Its commits then **land**:

- **Rebased** onto the target's tip if it moved. A conflict goes to a merge worker.
- **Fast-forwarded** onto your checked-out branch (local checkout) or the session branch
  `brigadier/<session>/session` (new worktree). Landing into your checkout never overwrites
  uncommitted, untracked or ignored files. If it would, or if the branch moved unexpectedly, the
  task waits as "ready to land" and nothing changes.
- **Reviewed** afterwards. Every landing gets one read-only review by the other vendor, in its
  own checkout, in the background. Nothing waits for it. Its findings go to the thread as a
  message, even after a merge, and the thread fixes them or says why not. Settings ›
  Configuration › "Review the work" turns reviews off (they're on by default).

A new-worktree session merges into its base only when you say so: on the merge card the thread
offers, or by asking in plain words.

Archiving a session removes its worktrees, scratch folders, CLI session files and processes,
including detached children. Unfinished work is kept as a WIP commit on its task
branch, or as a patch when it overlaps uncommitted changes you let workers see. Anything that
couldn't be removed is retried at the next launch.

### Permission levels

A session's permission level is picked in the composer, and a project remembers the last one
used. **A new project starts in Full access**; Settings › Configuration changes that default.

- **Ask for approval**: you approve every plan and every change before it lands. Workers run
  in the OS sandbox without network. Each step outside it (another folder, a host to reach)
  asks you on a card. The "Allow … for this session" button allows commands with the same first
  words (such as `git push` or `curl`), connections to the same host, or file changes inside
  the asker's own checkout or worktree for the rest of the conversation. File changes outside
  that folder, in Git metadata, or in the CLIs' own config (`.claude`, `.codex`, `.agents`,
  `.vscode`, `.mcp.json`) still ask. These grants live in memory only.
- **Approve for me**: Brigadier approves plans and changes for you (a risky plan gets an
  independent review first) and asks you only what only you can answer. Workers run in the OS
  sandbox with network. When a command needs more, the CLI's own reviewer decides (Claude's auto
  mode, Codex's auto-review) and declines what it judges unsafe.
- **Full access**: as Approve for me, but workers run like your own terminal (Claude in
  `bypassPermissions`, Codex with no sandbox and no approvals). They can read, change and delete
  any file your account can, run any command and use the internet, without asking. The composer
  shows a notice while a conversation is in Full access.

In the sandbox a worker can write to its own worktree and scratch folder, its worktree's git
folder (so it can commit) and the toolchains' caches (`~/.cargo`, `~/.rustup`, the npm, pnpm,
yarn and bun caches, `~/Library/Caches`, the temporary folder), so builds, tests and installs
just work. Claude's sandbox lets network traffic out only as HTTP(S) through its proxy, so a push
over `git://` or `ssh` from a sandboxed Claude worker fails.

At every level, workers never push, publish, deploy or open pull requests on their own: they
list such steps, and you start them. The thread asks for your approval before it spends money,
uses credentials or the keychain, or destroys something outside the session's own work.

### What still depends on trust

- Workers and threads get grants scoped to their role and task, checked on every call to
  Brigadier's MCP server and revoked when the session ends. Requests only the UI may make (such as
  answering approvals) need the IPC token in `<data>/run/`. Claude workers can't read that
  folder, and neither can read-only Codex sessions. A Codex worker that writes in a worktree
  runs in Codex's older sandbox, which can't deny reads, so a hostile one could read the token.
  On macOS it still can't act as the UI: the daemon refuses the token from any sandboxed process.
  On Linux and Windows, which have no worker sandbox yet, nothing refuses it.
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
from which session, model and commit. The thread asks it first, using full-text search plus local
embeddings, then one hop along the graph. It also keeps what you settle. Reports, plan decisions
and your answers on cards are recorded without being asked.

- **Code index.** The repository is scanned (tree-sitter symbols and references, manifests,
  scripts) and kept current by a file watcher. Workers search it before reading files.
- **Staleness.** Nodes remember the content hash of the files they describe. When a file
  changes, they're marked stale and answered as "may be outdated".
- **Spare quota.** After a project's first scan, the cheapest model maps its modules, stack,
  conventions and build recipe. Quota that would expire unused refreshes stale nodes and fills
  gaps, and it stops as soon as your own work starts.
- **Personal memory.** Preferences that hold in every project are given to new threads and Chats
  (Settings › Configuration › Memories).

Everything stays in the data folder: `brains/<project>/brain.sqlite`, `index.sqlite`,
`brains/personal.sqlite`, and the embedding model under `models/embeddings/`.

### Routing and quota

Brigadier picks each worker's model and says why on the worker's card.

- **Registry.** `registry/models.json` rates every model Brigadier drives: strengths per kind of
  work, efforts, context window. The app checks this repository for a newer revision daily. It
  takes one only if it stays within fixed bounds and has a higher revision.
- **Outcomes.** Every model's run of a task is recorded per project (result, review findings,
  rework, time, tokens), and nudges that model's score for that kind of work in that project.
- **Your choices.** Settings › Providers has one "Use this model" switch per model, and "Who does
  what" to pick a model per kind of work or leave it on Automatic. Ordering, rules and
  per-project overrides are under Advanced.
- **Quota.** Brigadier reads Claude's and Codex's usage windows, projects each one to its reset,
  and shifts new work away from a provider that's running hot.
- **Fewer tokens, built in.** Nothing to turn on. A thread idle past its prompt cache restarts
  from a checkpoint briefing instead of resending its whole history. A worker past 160k tokens
  hands over to a fresh session of the same model. Workers start without the built-in tools they
  never use. Brain answers return a few results of each kind and page the rest.

### Settings

Settings is grouped into Personal (General, Voice, Configuration, Keyboard shortcuts), Agents
(Providers, Usage, Git, Computer use), Browser & Data (Browser, Import Browser Data) and System
(Storage, Inspector), with Archived chats at the end. Computer use has its own guide:
[docs/COMPUTER-USE.md](docs/COMPUTER-USE.md). Accounts: [docs/ACCOUNTS.md](docs/ACCOUNTS.md).

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

## Contributing, security, license

- [CONTRIBUTING.md](CONTRIBUTING.md): read it before you open an issue or a pull request. Pull
  requests need an accepted issue first, and they're closed automatically without one.
- [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md): building, checks, layout and dependency notes.
- [SECURITY.md](SECURITY.md): report vulnerabilities privately, never in a public issue.
- [docs/](docs/README.md): design plans and the evidence behind the numbers above.
- MIT, see [LICENSE](LICENSE). Third-party notices: [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

Brigadier is made by SYNCRA SRL. Founded and developed by Stephen Golban.
