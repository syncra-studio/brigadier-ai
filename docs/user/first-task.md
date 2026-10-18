# Your first task

## Setup (about two minutes)

The first time you open Brigadier it walks you through four short screens.

1. **Welcome.** Press **Continue**.
2. **Your name.** What Brigadier calls you.
3. **Connect your agents.** Claude Code and Codex side by side, each **Connected** or
   **Not connected**. **Connect** installs the agent with its maker's own installer when it's
   missing, then opens its own sign-in in your browser; once you've signed in there it shows
   **Connected ✓**. You need at least one connected to continue; both is better, because each
   reviews the other's work.
4. **Choose your projects.** Brigadier suggests the repositories you have used with Claude Code or
   Codex lately. Pick some, **Add a folder…**, or **Continue without projects**.

You can run this again from **Settings › General › Run setup again**.

## Add a project

A project is a git repository on your Mac. In the sidebar, press **+** next to **Projects**:
**Add project** lists the git repositories on your Mac, most recent first. Pick one and press
**Add**, or:

- **Choose folder…**: any folder (Brigadier starts git in one that has none).
- **Clone from URL**: a repository to clone, into `~/code` unless you change where.
- **New repository**: a new, empty repository, named as you like.

The first time you open a project, Brigadier asks **Trust this folder?** Trust it to let agents
work on their own; if you don't, every session there asks you before each step.

## Start a task

1. Press **New session** (the **+** on the project's row in the sidebar). Right after you add a
   project, its new session is already open.
2. Say what you want in plain words, the way you would tell a colleague: "Add a dark mode toggle
   to the settings page".
3. Press Enter.

The session starts on its own branch, in its own copy of the repository (a worktree), so your
checkout is never touched. It reads the code, plans and makes the change; your project's checks
and a review by the other agent run in the background. Its row in the sidebar says what it is doing:
**Working**, **Needs you** when it has a question or an approval for you, and **Ready** when
the work is done and waiting to merge.

With [auto-merge](auto-merge-and-revert.md) on (the default), it then merges the work itself and
the row says **Merged**. A **Revert** button sits under the merge message if you change your mind.

## How much it asks you

The permission level is the pill in the composer. New sessions start in **Approve for me**:
Brigadier approves plans and changes for you, and asks only what only you can answer. **Ask for
approval** asks before every plan and change; **Full access** lets agents run like your own
terminal. Change the default in **Settings › Configuration**.

## Next

- Run several tasks at once: [Sessions and tabs](sessions-and-tabs.md).
- More than one Claude or Codex account: [Accounts](accounts.md).
