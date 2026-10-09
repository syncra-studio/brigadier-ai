# Several accounts per agent

The user can sign in to more than one Claude Code or Codex account, pick in Settings → Accounts
the one new chats start on (Use / In use), and let Brigadier move a chat to another account of
the same agent when one hits its limit. There is no account picker in the composer.
Evidence for every CLI behaviour relied on here: `docs/evidence/2026-10-09-accounts.md`.

## Design

- **Brigadier never handles a credential.** It runs the user's own `claude` and `codex`. Each
  extra account is a CLI home of its own, `<data dir>/accounts/<id>/`, passed as
  `CLAUDE_CONFIG_DIR` or `CODEX_HOME`. The CLI keeps that account's login there, as it would in
  `~/.claude` or `~/.codex`. Signing in is the CLI's own `claude auth login` or `codex login`,
  run in a terminal with that home. Who is signed in comes from the CLI's own status
  (`claude auth status --json`, Codex app-server `account/read`). Removing an account runs the
  CLI's own logout in that home, then deletes the folder.
- **The computer's own login is the first account.** It runs with no home variable, exactly as
  before, so the user's terminal and Brigadier share it. It can't be removed. Switching accounts
  in Brigadier never changes what the terminal's `claude` or `codex` is signed in to.
- **One chat history.** An account home links back to the main home's history (Claude:
  `projects`; Codex: `sessions`, `archived_sessions`, `generated_images`) and to the user's
  shared instructions and skills, so any account can resume any session. That is how a chat
  moves between accounts without losing its conversation, and the terminal's `claude --resume`
  and `codex resume` still list Brigadier's chats. `settings.json`, `.claude.json` and
  `history.jsonl` are not linked; the adapter reads the user's settings from the main home.
- **Login variables can't override a home.** Processes on an extra account drop
  `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`, `CLAUDE_CODE_OAUTH_TOKEN`, `OPENAI_API_KEY`,
  `CODEX_API_KEY` and `CODEX_ACCESS_TOKEN`, and Claude's `--settings` blanks the Claude ones, so
  the home's login is the one used. The own login keeps today's behaviour.
- **Quota per account.** The quota monitor and `routing.sqlite` (quota samples and turn usage)
  are keyed by account. Every signed-in account is polled through free reads only (Claude's
  `get_usage` control request, Codex's `account/rateLimits/read`); no model turn is spent on it.
  Routing and the Usage page show each agent's *lead* account: the one new work would start on.
  A model's own window (Claude's weekly Opus) used up on the lead is shown to routing as the
  window of the signed-in account with the most room in it, since work on that model starts
  there. An agent counts as signed in when its lead account is.
- **Which account work starts on** (`brigadier_core::accounts::select`): the agent's default
  account while it can take work; otherwise, with switching on, the signed-in account with the
  most left (per-model windows respected, under any of the model's names); otherwise the default (which then reads as limited,
  and routing moves the work to the other agent, as before).
- **When a chat hits a limit** with switching on and another account of the same agent left,
  the chat closes its CLI, records the new account in its setup and resumes the same session
  there. If the CLI had already begun the turn, it is told to continue rather than sent the
  message again, so nothing runs twice (a message that never reached the old CLI still goes).
  A note in the chat says what happened, in place of the CLI's own limit message. Only when every
  account of the agent is used up does the cross-agent fallback run. With switching off, the
  fallback runs at once. A worker hands its task to the next route as before, which now starts
  on the lead account; "Open in terminal" reopens a worker on the account its session ran on.
- **Settings** keep only names and ids: `Settings.accounts` (`id`, `provider`, `name`,
  `default`, `addedAtMs`), `Settings.switchAccounts` (on by default) and `ModelChoice.account`
  (an account id, `own` for the computer's login, or absent for the default).

## Files

| What | Where |
| --- | --- |
| Account homes, links, env overlay | `crates/providers/src/accounts.rs`, `CliEnv::for_account` |
| Per-account adapters | `Claude::for_account`, `Codex::for_account` |
| Account choice | `crates/core/src/accounts.rs` (`AccountRef`, `select`, `edited`) |
| Live accounts, polling, view | `crates/core/src/runtime/accounts.rs` |
| Per-account quota | `crates/core/src/routing/monitor.rs`, `routing/store.rs` |
| Add and remove | `crates/core/src/manager/accounts.rs` |
| Auto-switch | `crates/core/src/manager/conversation.rs` (`switch_account`), `fallback.rs` |
| IPC | `GetAccounts`, `AddAccount`, `SignInAccount`, `RemoveAccount`, `RefreshAccounts`; event `accountsChecked`; sign-in terminal in `crates/daemon/src/server.rs` (`open_account_terminal`) |
| Settings → Accounts | `apps/desktop/src/app/settings/AccountsPage.tsx`, `state/accounts.ts` (a sign-in already being opened is shared, so a double mount or click adds one account) |
| Tests | `crates/core/src/manager/flow/accounts_tests.rs`, `takeover_tests.rs` (account case), unit tests in the files above |

## How to test

- `cargo test -p brigadier-core --lib accounts` runs the flow tests: a limit moves a chat to
  another account and its work is done once; every account limited falls back to the other
  agent; switching off falls back at once; a one-click switch resumes the same session; two
  chats run on two accounts at once; a worker's task carries on with another account; removal
  is refused while an account is in use.
- `cargo test -p brigadier-providers accounts` covers homes, links and the env overlay.
- On a dev build, without any real sign-in: run a dev `brigadierd` on a scratch data dir with a
  scratch `HOME`, and with `ZDOTDIR` pointing at a zsh config that puts fake `claude` and `codex`
  scripts first on `PATH` (the daemon takes `PATH` from a login zsh). The fakes answer the CLIs'
  status, sign-in, quota and session requests, keyed on `CLAUDE_CONFIG_DIR` / `CODEX_HOME`, and
  log every call with its home. Drive the daemon with `tools/ab/bipc.py`. A file per account
  makes its turns end on a usage limit. The 2026-10-09 run is recorded in
  `docs/evidence/2026-10-09-accounts.md`.

## Signing in to a real second account

1. Settings → Accounts → **Add account** under Claude Code (or Codex).
2. Finish the sign-in the terminal row starts (`claude auth login` / `codex login` in the new
   account's own folder), in the browser, with the *other* account.
3. The row fills in with that account's email, plan and usage. Rename it from its menu.
4. Check that the terminal still uses the old account: `claude auth status` / `codex login status`
   in a terminal.
