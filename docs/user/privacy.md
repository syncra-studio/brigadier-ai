# Privacy

**Brigadier has no account, no server and no telemetry.** Everything it knows stays on your Mac.

## Your agents and logins

- Brigadier runs the `claude` and `codex` CLIs installed on your Mac, unmodified, with the logins
  you already have. It talks to them the way your terminal does.
- It never reads your login tokens or API keys, and holds none of its own. Signing in is the
  CLI's own sign-in (`claude auth login`, `codex login`).
- A second account gets its own CLI folder inside Brigadier's data folder, so your terminal's
  login never changes. See [Accounts](accounts.md).
- What you send the agents goes to Anthropic or OpenAI exactly as it would from your terminal,
  under their terms.

## What stays on your Mac

- Your sessions, chats, settings and each project's Brain live in
  `~/Library/Application Support/Brigadier`.
- The first screen suggests projects from your Claude Code and Codex history, read on your Mac.
- Dictation runs a speech model on your Mac; the audio never leaves it. Read aloud uses the
  system voice.
- Env files you mark as secrets (such as `.env.local`) are copied into a session's worktree,
  never committed, and their values are hidden in everything Brigadier records.

## What Brigadier itself fetches

- **Updates**: Brigadier's latest release on GitHub, and the newest version of each agent CLI
  from wherever it was installed from (its own updater, npm or Homebrew). A minute after launch,
  then every few hours.
- **Model list**: once a day, this repository's list of models and what each is good at.
- **Two small local models**, downloaded once from Hugging Face: the search model each project's
  Brain uses, and the speech model, the first time you use dictation.
- **Site icons** for links in agents' answers, from those sites.
- **Agent installs**, only when you click Install on the first screen: the CLI's official install
  script from Anthropic or OpenAI.

None of these requests carries anything about you, your code or how you use Brigadier.

## Removing everything

**Brigadier › Uninstall Brigadier…** (menu bar, or Settings › Storage) removes what Brigadier
made: worktrees, scratch folders, CLI session files, its data folder. See
[Install](install.md#uninstall).
