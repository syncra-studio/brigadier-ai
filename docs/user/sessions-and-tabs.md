# Sessions and tabs

**A session is one task.** It has one chat with an agent, on its own branch and its own copy of
the repository, so sessions never step on each other or on your checkout.

## The sidebar

The sidebar lists your projects, and under each project its sessions. Each row says what the
session is doing, and why when there is a reason:

- **Working**: for example · watching CI, · 2 workers, · waiting for reset 14:30.
- **Needs you**: · approval, · question, or · failed. These rows come first in their project, and
  the project shows how many need you.
- **Ready**: the work is done; · PR open or · proof attached says what is waiting.
- **Merging**, **Merged**, **Reverted**.

The same words show on the session's chat tab. When a session has workers running (the agents it
hands parts of the job to), its row folds them under a count: open it to see each worker and its
state, and click one to open the session with that worker shown.

## Run several tasks at once

Each parallel task is a new session.

- **New session** (the **+** on the project's row) starts one and opens it.
- **⌘Enter** in the composer, or **/** › **Start as a new session**, sends your message as a new
  session in the background and keeps you where you are.
- When you ask a session for something outside its task, it can offer **Start this as its own
  session?** in the chat. **Start as session** starts it in the background; **Open** then shows
  it.

Sessions in the same project know about each other: they see what the others are changing and
warn when two touch the same files. When one merges, the others move onto the new base.

## Tabs

The tab bar shows the open session's chat tab and that session's own tabs: terminals, browser
pages, files, side chats. **+** adds one.

Closing the chat tab doesn't stop the session. If it is still working or needs you, a short note
says so; its row in the sidebar opens it again.

## Agents in the browser

On macOS the thread and its workers can use the built-in browser themselves: open a page, read
it, click, type and take a screenshot. Their page is a normal browser tab of the session, with
your sign-ins, and it opens in the background. Switch to it to watch.

- **You take over** by using the page: a click, a key or a scroll pauses the agent. The tab says
  so, and **Continue** hands it back.
- **It asks you first** before it pays, signs in, fills a password, card or code field, or sends
  anything in your name. Its card waits for **Go ahead** or **Don't**.
- **Merging on its own** proves a change to a web page by opening the preview in such a tab,
  using the change and attaching the screenshot to the merge.

Windows doesn't have this yet, and Linux has no built-in browser.

## Chats

A chat outside any project (**New chat** at the top of the sidebar) is a plain conversation with
one model, with web search. It doesn't change any code.
