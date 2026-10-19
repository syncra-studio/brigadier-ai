# Your first task

## Setup (about two minutes)

The first time you open Brigadier it walks you through four short screens.

1. **Welcome.** Press **Continue**.
2. **Your name.** What Brigadier calls you.
3. **Connect your agents.** A card for Claude Code and one for Codex, connected ones first. A
   connected agent shows its plan and **Connected**. One found on your Mac but signed out offers
   **Sign in**, which opens its own sign-in in your browser; the card waits, then turns
   **Connected**. One that isn't installed offers **Install**, which opens its install guide.
   You need at least one connected to continue; both is better, because each reviews the other's
   work.
4. **Choose your projects.** Brigadier lists the repositories you have used with Claude Code or
   Codex, all ticked. Untick any you don't want (**Select none** clears them all), **Add
   folder…** for another, then **Add** them, or **Continue without projects**.

You can run this again from **Settings › General › Run setup again**.

## Add a project

A project is a git repository on your Mac. In the sidebar, press **+** next to **Projects**:
**Add project** lists the git repositories on your Mac, most recent first. Pick one and press
**Add** followed by its name, or:

- **Choose folder…**: any folder (Brigadier starts git in one that has none).
- **Clone from URL**: a repository to clone, into `~/code` unless you change where.
- **New repository**: a new, empty repository, named as you like.

The first time you open a project, Brigadier asks **Trust this folder?** **Trust folder** lets
agents work on their own; with **Don't trust**, every session there asks you before each step.

## Start a task

1. Press **New session** (the **+** on the project's row in the sidebar). Right after you add a
   project, its new session is already open.
2. Say what you want in plain words, the way you would tell a colleague: "Add a dark mode toggle
   to the settings page".
3. Press Enter.

The session starts on its own branch, in its own copy of the repository (a worktree), so your
checkout is never touched. It reads the code, plans and makes the change; your project's checks
and a review by the other agent run in the background. Its row in the sidebar says what it is doing, and why:
**Working**, **Needs you** when it has a question or an approval for you, and **Ready** when
the work is done and waiting to merge (more in [Sessions and tabs](sessions-and-tabs.md)).

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
