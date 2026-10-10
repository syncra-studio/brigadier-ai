# Brigadier

A free, open-source (MIT), local-first desktop app for long-running AI coding sessions. You talk
to one orchestrator; it plans, delegates to workers from the CLIs on your machine, reviews their
work and reports back. The design and build plan live in [docs/PLAN.md](docs/PLAN.md).

Status: Phase 3 (the orchestrator loop: sessions, workers in worktrees, landing, plain Chats).

## Layout

```
crates/        Rust workspace (see PLAN.md §3)
  core/          domain model, catalog projection, session manager
  store/         SQLite WAL event store (single writer, read pool), blob store
  ipc/           authenticated local IPC + protocol types (exported to TypeScript)
  sandbox/       OS abstraction: paths, private files, processes, credentials, shell, sandbox
  providers/     Provider trait, normalized events, Claude and Codex adapters, replay fixtures
  git/           git engine: worktrees, snapshots, candidates, guarded landing
  mcp-server/    the Brigadier MCP server the orchestrator and workers talk to
  router/        routing: model registry, quota forecasts, outcome learning, decide()
  daemon/        brigadierd
  …              crates for later phases
apps/desktop/  Tauri 2 shell (src-tauri/) and the React UI (src/)
registry/      curated model registry, published from this repository (Phase 5)
docs/          plan and design notes
```

## Requirements

- Rust 1.98.1 (pinned in `rust-toolchain.toml`; rustup installs it on first use)
- Node 24+ and pnpm 12.6.0 (`packageManager` in `package.json`)
- Tauri 2 platform prerequisites: https://v2.tauri.app/start/prerequisites/
- CMake 3.x or newer and libclang, to build whisper.cpp (dictation's engine; see below).
  macOS has libclang with the Xcode Command Line Tools; on Windows install LLVM, on Linux
  `libclang-dev`.

## Dependency notes

- `wry` (the desktop shell): the Browser tab's page is a plain wry webview laid over the tab, not
  a Tauri one. Tauri gives every webview it makes the app's IPC bridge, init scripts and custom
  protocols (and child webviews need its `unstable` feature); a web page must get none of that.
  The crate is pinned to the release `tauri-runtime-wry` uses, since two copies of wry would
  register the same native classes. Linux has no embedded page: its tab opens pages in the
  system browser.
- `objc2`, `objc2-foundation`, `objc2-web-kit`, `block2` (macOS, the releases wry is built on):
  the Browser tab's page gets its own WebKit UI delegate, because wry's grants every camera and
  microphone request. Ours denies them without a prompt, sends popups to the system browser and
  leaves file uploads to wry's. This module, `apps/desktop/src-tauri/src/browser_ui.rs`, is the
  app shell's WebKit exception to the workspace's `unsafe_code = "deny"` (the sandbox crate's OS
  calls are the only others): calling WebKit through objc2 needs `unsafe`. The `#[allow(unsafe_code)]` sits on that module alone, and each `unsafe` in it says
  why it holds. WebKit asks macOS for the microphone before it asks the delegate, so the page
  also gets a script, before any of its own and in every frame, that takes
  `navigator.mediaDevices`, `getUserMedia` and the speech-recognition APIs away for good
  (`NO_CAPTURE` in `browser.rs`). Windows' WebView2 asks the user itself.
- Overnight run notifications use a small macOS UserNotifications adapter under the app's
  bundle identity, retaining a stable notification ID and session/run activation payload.
  `overnight_notifications.rs::mac` is the shell's other narrowly scoped unsafe exception:
  its C strings live through each call, Objective-C copies them, and callback tickets carry
  no borrowed pointers. The desktop notification plugin handles the other platforms.
- `whisper-rs` (whisper.cpp), `ureq`, `sha2` (brigadierd): dictation turns speech into text on
  this computer; the audio never leaves it. The composer's webview records the microphone and
  streams 16 kHz PCM to the daemon, which feeds it to a short-lived `brigadierd transcribe`
  process, so the daemon never holds the model. The model (whisper.cpp's `ggml-base-q5_1`,
  60 MB) is downloaded from Hugging Face on first use, at a pinned revision, checked against its
  SHA-256 and kept in the data directory under `models/whisper/`. `.cargo/config.toml` builds
  whisper.cpp for any CPU of the target's kind (`GGML_NATIVE=OFF`); its Rust bindings are
  generated for the target, since the crate's bundled ones don't fit Windows. The approach
  follows OpenWhispr (MIT); no code is copied from it.
- `tree-sitter`, `tree-sitter-tags` and the language grammars, `ignore`, `notify`,
  `notify-debouncer-full`, `toml`, `yaml-rust2` (the code index, `crates/index`): no model
  involved. Symbols and references come from each grammar's tags query (C#, Swift and Kotlin
  get corrected copies under `crates/index/queries/`, and TypeScript and JavaScript the
  JavaScript query joined with the TypeScript one and our own patterns for exports, types,
  enums, class-field functions, top-level constants and JSX components; credited in
  `THIRD_PARTY_NOTICES.md`).
  References are matched to definitions by name, without type information.
- `tokenizers`, `safetensors`, `memmap2` (the Brain's embedder, `crates/brain`): the embedding
  model is `minishlab/potion-retrieval-32M` (Model2Vec static embeddings, MIT, 512 dimensions,
  about 130 MB), downloaded from Hugging Face once a project exists, at a pinned revision,
  checked against its SHA-256 and kept under `models/embeddings/`. Embedding is a token lookup
  and a mean, in the daemon; the model is loaded on first use and freed after 10 idle minutes.

## Develop

```sh
pnpm install
pnpm tauri:dev          # builds and stages brigadierd, starts Vite and the app
```

`pnpm tauri:dev` and `pnpm tauri:debug-app` (a debug `Brigadier Dev.app`) run as
`ai.brigadier.dev`, so they never hand a launch to an installed Brigadier. Any debug build
keeps its data in `/tmp/brigadier-dev` unless `BRIGADIER_DATA_DIR` names another directory.

The app launches `brigadierd` detached; closing the window keeps both running in the menu bar.
Quit from the menu-bar item (or Cmd+Q) to stop the daemon too.

Data lives in `~/Library/Application Support/Brigadier` (`%LOCALAPPDATA%\Brigadier`,
`~/.local/share/brigadier`). Set `BRIGADIER_DATA_DIR` to use a throwaway directory; each data
directory gets its own daemon.

## Uninstall

Use **Uninstall Brigadier…** when the app is still installed: it is in the
menu bar (Brigadier ▸ Uninstall Brigadier…), in Settings → Storage, and at the
bottom of Settings → Storage → Manage storage…. If the app has already been deleted, run the
standalone script from this repository:

```sh
scripts/uninstall.sh --dry-run --app-id ai.brigadier.app
scripts/uninstall.sh --app-id ai.brigadier.app
```

The script accepts `--keep-data`, `--data-dir PATH`, `--app-id ID`, `--app PATH`,
`--dev`, `--dry-run`, and `-h`. Its default data directory is
`$BRIGADIER_DATA_DIR` or `~/Library/Application Support/Brigadier`; the
default app path is `/Applications/Brigadier.app` for the default identifier.
The default identifier must be named explicitly. For one development instance,
pass `--dev --app-id ai.brigadier.<name> --data-dir PATH` (and `--app PATH` if
there is a bundle). `--dev` never sweeps other identifiers or data directories.

The script quits that app if it still runs, asks that instance's daemon to
quit, removes recorded clean git worktrees and merged Brigadier branches, and moves its data and exact per-app
folders to the Trash. Dirty worktrees and unmerged branches stay; the summary
prints a command for each one to resolve manually. It removes only
ledger-recorded Claude session files and Codex rollouts, and reports Codex
state database rows and trust entries it cannot confirm. A remaining worktree
keeps the data directory in place and makes the script exit nonzero. You can
also keep the data directory intentionally with `--keep-data`. Empty the Trash
to reclaim the space moved there.

## Verify

```sh
cargo fmt --all --check
cargo run -p brigadier-ipc --bin gen-ts      # regenerate TS bindings after protocol changes
pnpm typecheck && pnpm lint && pnpm build
pnpm --filter @brigadier/desktop stage-sidecar --debug
cargo clippy --workspace --all-targets -- -D warnings
```

`pnpm lint` includes `brigadier/no-raw-design-values`: component code may only use theme tokens
(no raw colors, arbitrary Tailwind values or numeric inline styles). Raw values live only in
`apps/desktop/src/styles/tokens.css`.

Launch smoke check against the performance budgets (PLAN.md §4):

```sh
pnpm tauri build
BRIGADIER_DATA_DIR=$(mktemp -d) target/release/bundle/macos/Brigadier.app/Contents/MacOS/brigadier --smoke
```

For a test build, use `pnpm tauri:debug-app` and `target/debug/bundle/macos/Brigadier Dev.app`.
The check needs the logged-in desktop session: inside a sandbox that blocks the window server, or
over SSH, it says so and exits with code 3.

It prints a JSON report (also written to `BRIGADIER_SMOKE_REPORT` if set) and exits non-zero if
a budget for an implemented feature is missed. `BRIGADIER_BUDGET_TOLERANCE` multiplies timing
budgets only; CI uses 3 on shared runners, locally it is 1.

Startup is judged against one of two budgets. A launch on a new data folder is a first launch:
no model lists are saved yet, so the startup screen waits for the signed-in agent CLIs to report
theirs (at most 3 s once the app has drawn), and it has 3 s. Any later launch has 1 s. Missing or
signed-out CLIs never hold the startup screen. The startup note breaks the time down into
milestones (ms since process start: webview, script, connected, catalog, paint, agents,
revealed), and the frame-gap note says when the longest gap fell and which event flush cost the
most.

In CI, after warm-up launches that are never judged, the app is launched in three pairs: a first
launch on a new data directory, then a second launch on the same directory.
`apps/desktop/scripts/judge-smoke.mjs` judges them:

- first launch by the **median** of the three first launches (3 s × tolerance), and cold start
  by the median of the three second launches (1 s × tolerance);
- every other check on the first launch, exactly as the app judged it.

All six reports, with their milestones, are printed in the log and kept in the combined
report artifact. This is because, on shared runners, most of a cold start passes before the page
even loads, while the platform creates the window and webview. That part swings by seconds
between identical launches, for example on the same code:

| Launch | webview | script | connected | catalog | paint (cold start) |
|---|---|---|---|---|---|
| macOS runner | 2299 | 2630 | 2661 | 2846 | 2909 |
| Windows runner | 3191 | 3329 | 3353 | 3590 | 3599 |
| Local Mac | 234 | 307 | 312 | 341 | 349 |

On Windows the same shell code measured anywhere from 1080 ms to 3599 ms across runs.

## Providers

Brigadier drives the `claude` and `codex` CLIs already installed and logged in on your machine,
found on your login shell's `PATH`. It never reads their credentials, and it changes their
config only to undo what a session made Codex add (see below). What a session makes a CLI write (Claude's transcript and task files, Codex's rollout) is
removed when the session is closed or archived.

- Claude Code runs as one persistent `claude -p --input-format stream-json --output-format
  stream-json` process per session.
- Codex runs as one `codex app-server` per session, over stdio JSON-RPC. The Rust bindings in
  `crates/providers/src/codex/protocol.rs` are generated from the installed CLI's schema. After
  a Codex upgrade, regenerate them:

  ```sh
  cargo run -p brigadier-providers --features codegen --bin gen-codex -- "$(command -v codex)"
  ```

The Inspector's **Providers** tab shows each CLI's login state, version, live model list and
remaining quota. From it you can start, resume, fork, steer, interrupt, stop and close raw
sessions, answer their approval requests, simulate a usage-limit event and replay recordings.
Recordings are scrubbed of personal data as they are written. The fixtures in
`crates/providers/fixtures/` ship with the app.

## Sessions

A session is one orchestrator (Claude or Codex) with no built-in tools except the Brigadier MCP
server (`brigadierd mcp`, bridged to the daemon's socket). Through it the orchestrator proposes
plans, delegates tasks, answers workers' questions, accepts or rejects reports and finishes the
session; only messages, reports, decisions and tool results enter its context, which the
Inspector's **Orchestrator** tab shows. Workers run in their own worktrees under Brigadier's data
directory, report through the same server, and never block the orchestrator. Every accepted
write task is reviewed by the other vendor, then lands as one commit (with your git identity) on
your checked-out branch (local checkout) or on the session branch `brigadier/<session>/session`
(new worktree), which lands on its base when the session finishes. Landing into your checkout is
a fast-forward only; if you switched branches, the tip moved unexpectedly or an untracked or
ignored file would be overwritten, the task waits as "ready to land" and nothing changes. A
review that fails or is stopped lands nothing: the task goes back to reported, and the
orchestrator is told so it can accept it again. A task that has landed or failed counts as
running for the orchestrator until its message is delivered, so a turn in between is never told
that nothing else is running.

Workers report through `submit_report`; the orchestrator never sees their messages. It can send
a worker back to fix something (`message_worker`), even while the worker is still finishing
the turn it reported in: the task reopens and its next report is taken. If an answer that sent a
worker back ends with nothing said, Brigadier asks the orchestrator for it once, and says so in
the thread if it still gives none. Each worker
has an outputs folder in its scratch folder for files meant for the orchestrator or you (full
findings, documents, generated images; a Codex worker's generated images are copied there as they
are made). What it leaves there, and every file its report names or mentions by path, is stored
with the report before the task's folders are removed, so the orchestrator can read it with
`read_artifact` after the task ended. A report that names a file that doesn't exist, one the
worker wrote to a temp folder, or a link out of its scratch folder, is refused with what to do
instead; a worker that writes its findings as a message rather than reporting them has that
message kept and attached to its report. The task card lists the outputs and artifacts with Open
and Save to….

Each project can name gitignored env files (for example `.env.local`) as secrets: they are copied
into each worker's worktree, never committed, and their values are redacted from everything
Brigadier records. Archiving a session removes its worktrees, scratch and temp folders (a Claude
worker's is a short `/tmp/brigadier-<id>`, as Claude's sandbox needs), CLI session files (Codex's
generated images for its threads included) and processes (the whole process tree, detached
children included). Unfinished work is kept as a WIP commit on its task branch. When it
overlaps uncommitted changes you let workers see, it is saved as a patch instead and its branch
is deleted, because your uncommitted changes never stay in a commit; the task card shows the
patch and can restore it as a new branch on the current tip of the target branch (or says where
it conflicts). A task branch with no work of its own, and an
archived session's branch that its base already contains, are deleted too. Anything that could
not be removed is retried at the next launch.

A project's menu in the sidebar has **Remove project…**. After a confirmation that says what
goes and what stays, its sessions are deleted with the same cleanup (workers stopped, worktrees,
scratch, CLI session files and processes removed), its Brain job stops, and its Brain and code
index are deleted from the data directory. Brigadier's unmerged session and task branches are
kept unless you choose to delete them, as when deleting a session. The repository itself (its
files, commits and your own branches) is never touched; adding the folder again starts afresh.

A Chat is a plain conversation with one model and no tools but web search. Text attachments up
to 200 kB go into the message itself (a Chat cannot read files); other attachments are noted.

A long paste (5,000 characters or more) shows as a "Pasted text" card in the composer and the
thread, but it is part of what you wrote: in a session or a Chat it goes to the model inside your
message, as if typed, and a message that is only a paste titles its conversation from the text.
Up to 200 kB goes whole; a longer paste goes as its first 150 kB and last 50 kB with a note of
what was left out, and its card says so. In a session the orchestrator can still give a worker
the whole paste as a file. Files you attach keep going to workers (or, in a Chat, inline) as files.

`BRIGADIER_ROUTE_CHEAP=1` in the daemon's environment makes every worker use its vendor's
cheapest model at low effort, keeping the routing reason and saying so. It works in development
builds only; a release build ignores it.

Development builds can also force a usage limit, to exercise fallback without spending real
quota: the Inspector's fault control does it for a running task or conversation, and
`BRIGADIER_FAULT=claude-limit[:tool-calls=N][:window=ID][:reset=MINUTES]` (or `codex-limit`)
does it for the first worker the daemon starts on that provider. After the session's next N
finished tool calls (3 by default) the quota monitor holds the provider at that limit until the
reset (60 minutes by default), the session's transcript notes the injection, its running turn is
interrupted, and when that turn ends the session reports the limit as a CLI at its limit would.
Neither exists in a release build.

### Permission levels

A session's permission level is picked in the composer; a project remembers the last one used.

- **Ask for approval**: you approve every plan and every change before it lands. Workers run
  in the OS sandbox without network; each step outside it (another folder, a host to reach)
  asks you on a card. The visible "Allow … for this session" button allows commands with the
  same first words (such as `git push` or `curl`), connections to the same host, or file changes
  inside the asker’s own checkout/worktree for the rest of the conversation, including worker
  handoffs. File changes outside that folder or in Git metadata still ask. These grants stay
  in memory only; nothing is saved across sessions. Full access is the persistent alternative.
- **Approve for me**: Brigadier approves plans and changes on your behalf (a risky plan gets an
  independent review first) and asks you only what only you can answer. Workers run in the OS
  sandbox with network; when a command needs more, the CLI's own reviewer decides (Claude's
  auto mode, Codex's auto-review) and declines what it judges unsafe, which the worker then
  works around or lists for you.
- **Full access**: as Approve for me, but workers run like your own terminal (Claude in
  `bypassPermissions`, Codex with no sandbox and no approvals). They can read, create, change
  and delete any file your account can, run any command (install software, change settings)
  and use the internet, without asking. The composer shows a notice while a conversation is in
  Full access; Settings can turn it off.

In the sandbox a worker writes its own worktree and scratch folder, its worktree's git folder
(so it can commit) and the toolchains' caches (`~/.cargo`, `~/.rustup`, the npm, pnpm, yarn and
bun caches, `~/Library/Caches`, the temporary folder), so builds, tests and installs just work.

At every level workers never push, publish, deploy or open pull requests on their own: they
list such steps for you, and you start them (with the session's buttons, or by asking the
orchestrator in chat). The orchestrator asks your approval only for spending money, using
credentials or the keychain, or destroying something outside the session's own work. An
overnight run follows its session's level. Claude workers' sub-agents run only on the models
their task may use, at every level.

Claude's sandbox lets network traffic out only as HTTP(S) through its proxy, so a push over
`git://` or `ssh` from a sandboxed Claude worker fails.

### What still depends on trust

- Workers and orchestrators get grants scoped to their role and task, checked on every MCP call
  and revoked when the session ends. UI-only requests (such as answering approvals)
  never accept a grant; they need the IPC token in `<data>/run/`. Claude workers cannot read that
  folder, nor can Codex sessions that run in a Brigadier folder (read-only workers and Brain
  jobs), which get a Codex permission profile denying it; a Brain job never runs on Codex
  without one. A Codex worker that writes in a worktree keeps Codex's older sandbox, which
  cannot deny reads (a permission profile there would make Codex trust your checkout in
  `~/.codex/config.toml`), so a hostile one can read the token. It still cannot act as the UI
  on macOS: the daemon refuses the token from any sandboxed process (it asks the kernel who
  connected), unless the daemon runs in a sandbox itself. On Linux and Windows, whose worker
  sandbox is not built yet, nothing refuses it.
- A process that detaches and moves out of Brigadier's folders escapes the cleanup.
- A Codex orchestrator runs read-only with every approval declined. It still has: exec
  (JavaScript in an isolate that can only call its tools), `apply_patch` (each patch is an
  approval request, and Brigadier declines it), the MCP resource tools, `clock__curr_time` and
  `request_user_input` (refused). There is no shell, sub-agent, image, web search or goal tool.
- Codex writes a trust entry into `~/.codex/config.toml` when a thread starts with a writable
  sandbox in a folder you never trusted. Brigadier starts every thread without a sandbox of its
  own and sets it per turn, so Codex writes none. If one ever appears for a folder Brigadier
  created, it is removed through Codex's config API when the session closes, and only while it
  is still exactly `trusted`.

## Project Brain

Each project has a Project Brain: what Brigadier knows about it, so a question answered once is
not scouted again. It is a graph of nodes (modules, services, file summaries, decisions,
conventions, contracts, reports, research), each with its provenance: where it came from (the
code index, the skeleton pass, enrichment, a worker's report, the orchestrator, you), which
session, task, worker model and commit. The orchestrator asks it first (`query_brain`: SQLite
FTS5 plus local embeddings, merged, then one hop along the graph) and keeps what you settle with
`remember`. Reports, plan decisions and your answers on cards are recorded without being asked.
A report's text artifacts (research notes, markdown or plain-text findings; not diffs, logs,
transcripts or binaries) come in with it, redacted like the report, up to 24 KB in parts of about
1.4 KB split at headings, each linked to the report and with its provenance, so a finding kept
in a file answers the next question too. A node not embedded yet (learned while the model was
unloaded) ranks by its full-text match in both lists. `read_report` and `read_artifact` also
reach the reports and artifacts of the project's other sessions, by the ids a Brain answer names;
nothing outside the project.

- **Code index.** A project's repository is scanned on a thread of its own (tree-sitter symbols
  and references, manifests, scripts, services), then kept current by a file watcher. Workers
  search it with `code_search`, `code_refs` and `project_map`.
- **Staleness.** Nodes remember the content hash of the files they describe; when a file
  changes, they are marked stale and answered as "may be outdated".
- **Skeleton pass and enrichment.** After a project's first scan the cheapest model maps its
  modules, stack, conventions and build/run/verify recipe, read-only and sandboxed. With
  Settings → Personalization → "Use spare quota to deepen the Brain" (on by default), quota that would expire
  unused (a usage window resetting within the hour with 40% left, the provider idle for 10
  minutes) refreshes stale nodes and fills gaps; it stops as soon as your own work starts.
- **Rebirth.** The orchestrator never compacts its context. Past about 150k tokens its session
  is forked to write a handoff note; between two turns a fresh CLI takes over from a briefing of
  15–25k tokens (the note, every decision of the session, the live board, a Brain digest and
  the latest messages verbatim), and `search_transcript` reaches the rest. You see one
  conversation; the Inspector's Orchestrator tab has the rebirth log, which times each rebirth's
  work (the handoff note and the swap) apart from the wait for the next turn.
- **Personal Brain.** Preferences that hold in every project, from the orchestrator or from a
  Chat (`save_memory`, shown as Memory chips), are given to new orchestrators and Chats.
  Settings → Personalization lists them. A project's conventions can be exported to a marked section of its
  `AGENTS.md`.

Files: `brains/<project>/brain.sqlite` and `index.sqlite`, `brains/personal.sqlite`, and the
model under `models/embeddings/`, all in the data directory. The index is a cache and can be
rebuilt. Plan note: PLAN.md's cloud embedding fallback is not built, because Brigadier holds no
API keys; without the model, retrieval is FTS5 plus the graph.

## Routing and quota

Brigadier picks each worker's model itself; the card says why ("why this model", with the
factors in its details).

- **Registry.** `registry/models.json` rates every model Brigadier drives: tier, strengths per
  task category, area modifiers, efforts, context window and modalities. The app ships a copy
  and the daemon checks this repository for a newer revision a minute after launch and then
  daily (the Usage page can ask now). A download is taken only if it reads within fixed bounds
  (no Fable, efforts up to `high`, only CLIs and capabilities Brigadier drives) and its revision
  is higher; it is cached in `cache/registry/` in the data directory. It is not signed yet.
  Development builds can point `BRIGADIER_REGISTRY_URL` at another HTTPS address or at plain
  HTTP on 127.0.0.1 or ::1, to try an update against a local copy.
- **Ranking refresh.** The daemon can research current model cards, release notes and benchmarks
  on demand through an enabled CLI model. Sourced rating patches persist until reset or a newer
  registry revision supersedes them; manual rankings, routing rules and trial gates stay in force.
- **New models.** A model a CLI lists that the registry doesn't know is researched once, on
  spare quota (the Brain enrichment setting), from its release notes and benchmarks, and gets
  one in five scouting, research and verify tasks as a trial until it has three outcomes.
- **Outcomes.** Every model's run of a task is recorded per project in `routing.sqlite`
  (result, first review, rework, checks, time, tokens, quota share), and nudges that model's
  score for that kind of task in that project.
- **Your routing.** Settings → Routing shows, for each kind of work, the models routing would
  try next and why (Automatic), or lets you order them yourself with an effort for each
  (Manual): the list is tried top-down, and with "Wait for these" work waits for those models
  rather than going to others. Everywhere or per project, with area overrides. Rules there
  (never, prefer or only a model, family or vendor) keep models from work, during fallback too.
  No Fable, effort at most high, limits and cross-vendor review hold whatever you choose.
- **Workers' sub-agents.** A Claude worker's own sub-agents run only on models its task could
  have gone to (your rules, hidden models, quality floors and limits included, never Fable).
  When Claude can't be held to exactly those, the worker has no sub-agents. Codex workers have
  none, since Codex can't limit which model a sub-agent uses.
- **Quota.** The daemon reads Claude's and Codex's usage windows (every 5 minutes while work
  runs, every 30 when idle, and live from the sessions), keeps a week of samples, and projects
  each window to its reset. New work shifts away from a provider whose window runs hot. The
  Usage page (Settings → Usage, the rail's menu, or the status bar's chip) shows the windows, their estimates, Brigadier's
  own tokens, hand-offs and waits, and the models with what routing learned.
- **Less usage, built in.** There is nothing to turn on. A Claude orchestrator idle past its
  one-hour prompt cache starts over from a checkpoint briefing instead of sending its whole
  history again. A worker whose context passes 160k tokens hands over to a fresh session of the
  same model, with a handoff note and its full transcript on disk. Claude workers start without
  the built-in tools they never use, workers find code with the code index first, and Brain
  answers give a few results of each kind and page the rest. The orchestrator and the workers'
  reports are brief and plain and keep every fact; implement workers follow short code rules
  (reuse what exists, no extra abstractions, update every caller). See PLAN.md §7.
- **Fallback.** A worker whose provider hits a limit (or that keeps failing) hands its task to
  the best eligible model in the same worktree, with the spec, a progress log and the current
  diff; with none left the task waits for the earliest reset while others go on. A task the
  orchestrator gave to one vendor, or that needs image generation, goes only to a model that
  qualifies, on hand-off and on resume alike. An orchestrator or Chat continues on the other
  vendor and goes back after the reset; your saved model choices are never changed by it.

## Build and sign (macOS)

```sh
pnpm tauri build --target universal-apple-darwin
```

The bundle is `ai.brigadier.app`, universal, macOS 14+, hardened runtime. Signing and
notarization are configured only through environment variables and are skipped when unset:

| Variable | Purpose |
|---|---|
| `APPLE_SIGNING_IDENTITY` | Developer ID Application identity |
| `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD` | base64 .p12 and its password (CI) |
| `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` | notarization with an app-specific password |
| `APPLE_API_KEY`, `APPLE_API_ISSUER`, `APPLE_API_KEY_PATH` | notarization with an App Store Connect key |

CI reads the same names from repository secrets.

## License

MIT. Third-party notices: [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
