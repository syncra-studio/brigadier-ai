# Accounts

Brigadier uses the logins your `claude` and `codex` CLIs already have. You don't make a
Brigadier account.

## Your own login

The account your terminal's `claude` or `codex` is signed in to is the first account. Brigadier
and your terminal share it. If a CLI isn't signed in yet, Brigadier's first screen and
**Settings › Providers** offer **Sign in**: it runs the CLI's own sign-in (`claude auth login`,
`codex login`).

## More than one account

Open **Settings › Providers**, pick the agent, and under **Accounts** choose **Add another Claude
Code account** (or Codex). It signs in with the CLI's own sign-in and keeps
that login in a folder of its own inside Brigadier's data folder, so your terminal's login never
changes. Your terminal's own login shows as **This computer's login**.

- **Which one new work uses.** The one marked **In use**; choose **Use** on another row to change it.
  With **Switch to another account automatically** on (Settings › Providers, on by default),
  Brigadier picks the account whose weekly limit resets soonest, so quota that would be lost
  first is used first.
- **When one runs out** (with switching on). A chat that hits its limit moves to another account
  of the same agent and carries on in the same conversation, with a note saying so. When every account of that agent
  is out, the work moves to the other agent until the reset.
- **One history.** All accounts share the chat history, so `claude --resume` and `codex resume` in
  your terminal still list Brigadier's chats.

**Remove** in a row's menu signs that account out with the CLI's own logout and deletes its folder.
Your terminal's login can't be removed here.

## Usage

The usage button at the bottom of the sidebar lists your agents, the most used first, each with
its tightest limit: how much is used and when it resets (amber past 75%). Click an agent to see
every limit it has. **Providers ›** opens **Settings › Providers**.
