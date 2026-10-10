# Computer use

Brigadier's workers can see and use apps on your Mac: the apps they build, and any other app a task needs. They work
in the background. Your cursor and keyboard stay yours, and the app they use doesn't come to the front.

This page is the short version. The design and its measurements are in
[COMPUTER-USE-PLAN.md](COMPUTER-USE-PLAN.md).

## What it does

- **Every worker can take a quick look.** A worker that changes your app can open it, check a screen, press a
  button and read the result, without asking you.
- **Longer jobs go to an operator.** When a job needs many steps in an app, or exploring one, the thread hands it to
  an operator: a worker that only uses apps. It gets the target, the end state to reach, and reports what it did
  and how it checked the result.
- **It reads apps before it looks at them.** A worker reads an app's controls by name, so it can press, fill and
  pick them even when the window is behind others or on another desktop. It takes a screenshot only when the app
  doesn't describe itself well, such as a canvas or a game.
- **Web pages too.** In a browser the session opened, pages are read and used through the browser itself. In your
  own browsers, a page is read through what the browser tells the system about it.
- **You see what it does.** Each worker has its own coloured cursor on screen, drawn over the app it is using. Your
  real pointer doesn't move.
- **Every step is recorded.** The conversation shows each action, what it aimed at, and a screenshot with the point
  marked. Deleting the conversation deletes them.

It works with the Claude and Codex command-line tools you already use. Brigadier gives them the computer tools; it
doesn't run a model of its own.

## Turning it on

macOS asks you once for two permissions, for an item named **Brigadier Computer Use** (Brigadier's icon, listed
apart from Brigadier itself):

1. Open Brigadier's **Settings → Computer use**.
2. Next to **Control apps**, press **Allow…**. macOS shows its own prompt, then System Settings opens at Privacy &
   Security → **Device Control and Data Access** (macOS 27's name for the list called Accessibility before). Turn on
   **Brigadier Computer Use**.
3. Next to **See the screen**, press **Allow…**. System Settings opens at Privacy & Security → Screen & System Audio
   Recording. Turn on **Brigadier Computer Use**. If macOS offers **Quit & Reopen**, you can choose it or not:
   Brigadier Computer Use restarts by itself to use the new permission.

You don't need to come back and click anything: the page reads the permissions every second or two while one is
missing, and each row turns to **Allowed** on its own. It keeps reading them every few seconds while it is open, so a
permission you turn off later shows as **Not allowed yet** too, and a worker that asks for it is told it's missing.

The permissions belong to Brigadier Computer Use, not to Brigadier or a terminal, and they stay when Brigadier
updates. If a worker needs them before you've given them, the conversation shows the same two Allow buttons.

To open those lists yourself:

```sh
open "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"   # Device Control and Data Access
open "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"   # Screen & System Audio Recording
```

**On, but still "Not allowed yet"?** The switch belongs to an older build of Brigadier Computer Use: macOS ties a
grant to the build it was given to, and an unsigned build changes with every rebuild. After you've pressed **Allow…**
once, the row also offers **Start over**. It makes Brigadier Computer Use forget its own entry for that permission
(only its own, with macOS's `tccutil reset`), then macOS asks again; turn the new entry on. Signed builds keep their
grants across rebuilds and updates.

### Opening the dev build (for Brigadier's developers)

A checkout's dev build is `target/debug/bundle/macos/Brigadier Dev.app`. Its helper is named **Brigadier Computer
Use** too, with its own id (`ai.brigadier.dev.computer-use`), so it gets its own entries in System Settings. Build it
signed, so the grants survive rebuilds, then open it with the script, never `/Applications/Brigadier.app`:

```sh
cd apps/desktop && APPLE_SIGNING_IDENTITY="Developer ID Application: SYNCRA, SRL (7JQSPMWT79)" pnpm tauri:debug-app
cd ../.. && tools/open-dev-app.sh              # its own data, /tmp/brigadier-dev
tools/open-dev-app.sh /tmp/my-scratch          # or another data folder
```

## Safety

- **Some apps are never touched.** Workers can't see or use password managers, Keychain Access, the system's
  password and security prompts, or the Privacy & Security settings (each of its lists too, such as Device Control
  and Data Access), Users & Groups, Passwords and Login Items. They can't use terminal windows they didn't open, or the Brigadier you're using.
- **Passwords stay hidden.** A worker never reads a password field's contents, and Brigadier refuses to type or
  paste while a password field has the focus.
- **You come first.** If you're typing or clicking in the window a worker wants to change, it waits until you
  pause.
- **Stop it at any time.** Choose **Stop computer use** from the Brigadier Computer Use icon in the menu bar, or
  press **⌃⌥⌘.** (Control-Option-Command-period). Every worker stops at once and lets go of any key or button it
  was holding. Stopping a session in Brigadier does the same for its workers.
- **Operators stay inside their task.** They use only the computer tools and the files and apps their task names.
  They never stop other programs or search your whole disk.

## Limits

- **macOS only.** Windows and Linux are a future build.
- **Several displays.** Tested on a second 1× display beside a 2× Retina display.
- **Save dialogs on another desktop.** When a save dialog's window is on another desktop, or you're in a
  full-screen app, the dialog keeps its Save button disabled. The worker can't finish that save until the window is
  on your current desktop, and says so.
- **Chrome on another desktop.** Chrome and other Chromium browsers build no page for a window you can't see, so a
  worker can't read a page whose window is on another desktop until it's in view.
- **First look takes a moment.** Some apps (Electron apps, browsers, apps just launched) build their description
  only when asked. The first look at such a window waits for it, up to a few seconds.
