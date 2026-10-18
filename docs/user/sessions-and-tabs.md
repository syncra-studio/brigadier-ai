# Sessions and tabs

**A session is one task.** It has one chat with an agent, on its own branch and its own copy of
the repository, so sessions never step on each other or on your checkout.

The sidebar lists your projects, and under each project its sessions. Each row says what the
session is doing: **Working**, **Needs you**, **Ready**, **Merging** or **Merged**.

## Run several tasks at once

Each parallel task is a new session.

- **New session** (the **+** on the project's row) starts one and opens it.
- **⌘Enter** in the composer starts your message as a new session in the background and keeps you
  where you are.

Sessions in the same project know about each other: they see what the others are changing and
warn when two touch the same files. When one merges, the others move onto the new base.

## Tabs

The tab bar shows the open session's chat tab and that session's own tabs: terminals, browser
pages, files, side chats. **+** adds one.

Closing the chat tab doesn't stop the session: it keeps working, and its row in the sidebar opens
it again.

## Chats

A chat outside any project (**New chat** at the top of the sidebar) is a plain conversation with
one model, with web search. It doesn't change any code.
