# Your first task

## Setup (about two minutes)

The first time you open Brigadier it walks you through four short screens.

1. **Welcome.** Press **Continue**.
2. **Your name.** What Brigadier calls you.
3. **Connect your agents.** Claude Code and Codex side by side. Each shows **Install**,
   **Sign in** or **Connected ✓**. Install runs the agent's official install script; Sign in runs
   its own sign-in. You need at least one connected to continue; both is better, because each
   reviews the other's work.
4. **Choose your projects.** Brigadier suggests the repositories you have used with Claude Code or
   Codex lately. Pick some, **Add a folder…**, or **Continue without projects**.

You can run this again from **Settings › General › Run setup again**.

## Add a project

A project is a git repository on your Mac. In the sidebar, press **+** next to **Projects**:

- **Folder on this computer**: a repository you have, or a plain folder (Brigadier can start git
  in it, or create the folder).
- **Clone from URL**: a repository to clone.

The first time you open a project, Brigadier asks **Trust this folder?** Trust it to let agents
work on their own; if you don't, every session there asks you before each step.

## Start a task

1. Press **New chat** on the project in the sidebar.
2. Say what you want in plain words, the way you would tell a colleague: "Add a dark mode toggle
   to the settings page".
3. Press Enter.

The session starts on its own branch, in its own copy of the repository (a worktree), so your
checkout is never touched. It reads the code, plans, makes the change, runs your project's
checks, and has the other agent review it. Its row in the sidebar says what it is doing:
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
