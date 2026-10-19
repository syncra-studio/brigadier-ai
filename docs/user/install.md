# Install Brigadier

Brigadier runs on macOS 14 or later, on Apple silicon and Intel Macs. It is signed and notarized
by Apple, so it opens like any other app.

You also need at least one of the two agent CLIs: [Claude Code](https://docs.anthropic.com/en/docs/claude-code)
or [Codex](https://github.com/openai/codex). Both is better: each one reviews the other's work.
If they aren't installed yet, Brigadier's setup can install them for you.

## With Homebrew

```sh
brew install --cask syncra-studio/tap/brigadier
```

## Download

1. Download `Brigadier-0.1.0-alpha-universal.dmg` from the
   [latest release](https://github.com/syncra-studio/brigadier-ai/releases/latest).
2. Open it and drag **Brigadier** to **Applications**.
3. Open Brigadier from Applications.

## One command

```sh
curl -fsSL https://raw.githubusercontent.com/syncra-studio/brigadier-ai/v0.1.0-alpha/scripts/install.sh | sh
```

It downloads the same DMG, checks its checksum, checks that the app is signed by Brigadier's
developer and notarized by Apple, and copies it to Applications. It asks before it replaces
an existing Brigadier. To read it first, download it and run `sh install.sh`.

## Updates

Brigadier checks for new versions by itself, but it asks before it updates. When a newer
release is out, an update pill shows at the bottom of the sidebar. Click it to download the
update; once it's ready the pill turns into a restart pill (**Restart to update**) that restarts
into the new version. If you quit
instead of restarting, from the menu, with ⌘Q or from the Dock, the update installs as it quits.
Nothing installs until you click the update pill.

## Next

[Your first task](first-task.md).

## Uninstall

Use **Brigadier › Uninstall Brigadier…** in the menu bar, or **Settings › Storage**. It stops
every session, keeps unfinished work as commits on their branches, and removes the worktrees,
scratch folders and session files Brigadier made. Then move the app to the Trash (or run
`brew uninstall --cask brigadier`).
