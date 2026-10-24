//! `read_page` and `write_page`: the thread works on the page the user wrote from.
//!
//! A message sent from a page's composer carries that page as a [`Mention::Page`]. While it is
//! the user's latest message, the thread may read and change that one page; any other page, or
//! any page once the user writes again without one, is refused. The grant is checked on every
//! call against the conversation as stored, so it moves only when a message is actually sent
//! (a queued one counts once it is delivered) and holds for a session that was already running.
//! Writes go through the pages store: a page changed since the revision the thread read is
//! refused, never overwritten.

use super::SessionManager;
use crate::model::{ConversationId, Mention, MessageRole, TabChat, TabTarget};
use crate::pages::{PageContent, PageWriteOutcome, Pages};
use crate::tools::{
    ChatCall, ReadPage, SaveReport, TabChatCall, ToolReply, WritePage, WriteTabFile,
};
use crate::work::WorkspaceTarget;
use crate::{Error, Result};
use std::sync::Arc;
use tokio::sync::broadcast;

/// The page a user message was sent from, if any.
pub(crate) fn page_of(mentions: &[Mention]) -> Option<(&str, &str)> {
    mentions.iter().find_map(|mention| match mention {
        Mention::Page { id, title } => Some((id.as_str(), title.as_str())),
        _ => None,
    })
}

/// The note the thread reads under a message sent from a page.
pub(crate) fn page_note(id: &str, title: &str) -> String {
    let name = if title.trim().is_empty() {
        "Untitled page"
    } else {
        title.trim()
    };
    format!(
        "\n[page {id} \"{name}\"] Sent from this page of the user's: read it with `brigadier page read {id}` and change it with `brigadier page write {id}`. Only this page, and only until the user writes again."
    )
}

impl SessionManager {
    /// How the daemon opens a page in the user's browser, once.
    pub fn set_browser(&self, open: Arc<dyn Fn(&str) + Send + Sync>) {
        let _ = self.sign_in_browser.set(open);
    }

    /// The pages folder the daemon shares, once.
    pub fn set_pages(&self, pages: Arc<Pages>) {
        let _ = self.pages.set(pages);
    }

    /// The page the user's latest message on the branch shown was sent from.
    async fn granted_page(&self, id: &ConversationId) -> Result<Option<String>> {
        let messages = match self.core.head(id).await? {
            Some(head) => self.core.branch(id, &head).await?,
            None => self.core.all_messages(id).await?,
        };
        Ok(messages
            .iter()
            .rev()
            .find(|message| message.role == MessageRole::User)
            .and_then(|message| page_of(&message.mentions).map(|(page, _)| page.to_owned())))
    }

    /// The pages folder, if `page_id` is the page the user's latest message came from.
    pub(super) async fn page_access(
        &self,
        id: &ConversationId,
        page_id: &str,
    ) -> Result<Arc<Pages>> {
        match self.core.conversation(id)?.tab_chat {
            // A tab's chat has its tab's page, always, and no other.
            Some(TabChat {
                target: TabTarget::Page { page_id: granted },
                ..
            }) if granted == page_id => {}
            Some(TabChat {
                target: TabTarget::Page { .. },
                ..
            }) => {
                return Err(Error::Invalid(format!(
                    "page {page_id} isn't the page beside this chat: only that page may be read or changed"
                )));
            }
            Some(_) => {
                return Err(Error::Invalid(
                    "this chat isn't beside a page: no page may be read or changed".into(),
                ));
            }
            None => match self.granted_page(id).await? {
                Some(granted) if granted == page_id => {}
                Some(granted) => {
                    return Err(Error::Invalid(format!(
                        "page {page_id} isn't the page the user is writing from (that is page {granted}): only that page may be read or changed"
                    )));
                }
                None => {
                    return Err(Error::Invalid(
                        "the user's latest message wasn't sent from a page: no page may be read or changed".into(),
                    ));
                }
            },
        }
        self.pages
            .get()
            .cloned()
            .ok_or_else(|| Error::Invalid("pages aren't available".into()))
    }

    pub(super) async fn read_page(&self, id: &ConversationId, args: ReadPage) -> Result<String> {
        let pages = self.page_access(id, &args.page_id).await?;
        let page_id = args.page_id;
        let page = super::blocking(move || {
            pages
                .read(&page_id)
                .map_err(|err| Error::Invalid(err.to_string()))
        })
        .await?;
        let title = if page.content.title.is_empty() {
            "(untitled)"
        } else {
            page.content.title.as_str()
        };
        Ok(format!(
            "Page {} · revision {}\nTitle: {title}\n\n{}",
            page.id, page.revision, page.content.body
        ))
    }

    pub(super) async fn write_page(&self, id: &ConversationId, args: WritePage) -> Result<String> {
        let pages = self.page_access(id, &args.page_id).await?;
        let base = args.base_revision.clone();
        let outcome = super::blocking(move || {
            let current = pages
                .read(&args.page_id)
                .map_err(|err| Error::Invalid(err.to_string()))?;
            let content = PageContent {
                title: args
                    .title
                    .map(|title| title.replace('\n', " ").trim().to_owned())
                    .unwrap_or(current.content.title),
                emoji: current.content.emoji,
                body: args.body,
            };
            pages
                .write(&args.page_id, &content, &args.base_revision)
                .map_err(|err| Error::Invalid(err.to_string()))
        })
        .await?;
        match outcome {
            PageWriteOutcome::Saved { revision } => Ok(format!(
                "Saved: the page is at revision {revision}. It shows in the user's open tab."
            )),
            PageWriteOutcome::Conflict { current } => Err(Error::Invalid(format!(
                "not saved: the page changed since revision {} (it is at revision {} now). Read it again and redo your change on what it holds now.",
                base, current.revision
            ))),
        }
    }
}

impl SessionManager {
    /// A Chat's tool call.
    pub(super) async fn chat_call(&self, call: ChatCall) -> ToolReply {
        let result = match call {
            ChatCall::SaveReport(args) => self.save_report(args).await,
        };
        match result {
            Ok(text) => ToolReply::ok(text),
            Err(err) => ToolReply::error(err.to_string()),
        }
    }

    /// `save_report`: a research answer's full report as a new page of the user's, and the link
    /// its answer gives (Q79). Any thread or Chat may make one; it reads and changes no other page.
    pub(super) async fn save_report(&self, args: SaveReport) -> Result<String> {
        let title = args.title.replace('\n', " ").trim().to_owned();
        if title.is_empty() || args.markdown.trim().is_empty() {
            return Err(Error::Invalid(
                "give the report a `title` and its text in `markdown`".into(),
            ));
        }
        let pages = self
            .pages
            .get()
            .cloned()
            .ok_or_else(|| Error::Invalid("pages aren't available".into()))?;
        let page = super::blocking(move || {
            let page = pages
                .create()
                .map_err(|err| Error::Invalid(err.to_string()))?;
            let content = PageContent {
                title,
                emoji: None,
                body: args.markdown,
            };
            match pages
                .write(&page.id, &content, &page.revision)
                .map_err(|err| Error::Invalid(err.to_string()))?
            {
                PageWriteOutcome::Saved { .. } => Ok(page.id),
                PageWriteOutcome::Conflict { .. } => Err(Error::Invalid(
                    "the new page changed before the report was saved; save it again".into(),
                )),
            }
        })
        .await?;
        Ok(format!(
            "Saved as a page. Link it in your answer as [Full report](brigadier-page:{page}): one click opens it."
        ))
    }
}

impl SessionManager {
    /// The checkouts a tab chat saved a file in: the file's open tab reads it again, even while
    /// no sidebar watches the checkout.
    pub fn subscribe_checkout_saves(&self) -> broadcast::Receiver<WorkspaceTarget> {
        self.checkout_saves.subscribe()
    }

    /// A tab chat's tool call: its tab's page or file, and nothing else.
    pub(super) async fn tab_chat_call(&self, id: ConversationId, call: TabChatCall) -> ToolReply {
        let result = match call {
            TabChatCall::ReadPage(args) => self.read_page(&id, args).await,
            TabChatCall::WritePage(args) => self.write_page(&id, args).await,
            TabChatCall::ReadFile => self.read_tab_file(&id).await,
            TabChatCall::WriteFile(args) => self.write_tab_file(&id, args).await,
        };
        match result {
            Ok(text) => ToolReply::ok(text),
            Err(err) => ToolReply::error(err.to_string()),
        }
    }

    /// The tab chat's file: its checkout, its path inside it and the session it is beside.
    fn tab_file(
        &self,
        id: &ConversationId,
    ) -> Result<(std::path::PathBuf, String, ConversationId)> {
        let chat = self.core.conversation(id)?;
        let (
            Some(TabChat {
                target: TabTarget::File { path },
                ..
            }),
            Some(session),
        ) = (chat.tab_chat, chat.side_of)
        else {
            return Err(Error::Invalid(
                "this chat isn't beside a file: no file may be read or changed".into(),
            ));
        };
        Ok((self.checkout_dir(&session)?.into(), path, session))
    }

    async fn read_tab_file(&self, id: &ConversationId) -> Result<String> {
        let (root, path, _) = self.tab_file(id)?;
        super::blocking(move || {
            let full = tab_file_in(&root, &path)?;
            let text = std::fs::read_to_string(&full)
                .map_err(|err| Error::Invalid(format!("couldn't read {path}: {err}")))?;
            Ok(format!(
                "{path} at revision {}:\n{text}",
                file_revision(text.as_bytes())
            ))
        })
        .await
    }

    async fn write_tab_file(&self, id: &ConversationId, args: WriteTabFile) -> Result<String> {
        let (root, path, session) = self.tab_file(id)?;
        let saved = super::blocking(move || {
            let full = tab_file_in(&root, &path)?;
            save_checked(&full, &path, &args.base_revision, args.text.as_bytes())
        })
        .await?;
        // The file's tab reads it again, even with no sidebar watching the checkout.
        let _ = self.checkout_saves.send(WorkspaceTarget::Conversation {
            conversation_id: session,
        });
        Ok(saved)
    }
}

/// Saves `text` as `full` (the checkout's `path`) if it is still at `base_revision`, checked
/// again as it goes in place, so a write that lands meanwhile is never overwritten. Saves are
/// one at a time, so of two chats saving on the same revision, one gets a conflict.
fn save_checked(
    full: &std::path::Path,
    path: &str,
    base_revision: &str,
    text: &[u8],
) -> Result<String> {
    save_checked_with(full, path, base_revision, text, || {})
}

/// [`save_checked`], with `interfere` run after the revision check (tests change the file there).
fn save_checked_with(
    full: &std::path::Path,
    path: &str,
    base_revision: &str,
    text: &[u8],
    interfere: impl FnOnce(),
) -> Result<String> {
    // Few and quick: one lock for every tab chat's file saves.
    static SAVES: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one = SAVES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let unreadable = |err: std::io::Error| Error::Invalid(format!("couldn't read {path}: {err}"));
    let conflict = |current: &str| {
        Error::Invalid(format!(
            "not saved: {path} changed since revision {base_revision} (it is at revision {current} now). Read it again with `brigadier page file-read` and redo your change on what it holds now."
        ))
    };
    let now = std::fs::read(full).map_err(unreadable)?;
    let current = file_revision(&now);
    if current != base_revision {
        return Err(conflict(&current));
    }
    if let Err(reason) = super::search::replace_checked(full, &now, text, interfere) {
        // Changed meanwhile: a conflict like any other.
        let current = file_revision(&std::fs::read(full).map_err(unreadable)?);
        return Err(if current == base_revision {
            Error::Invalid(format!("couldn't save {path}: {reason}"))
        } else {
            conflict(&current)
        });
    }
    Ok(format!(
        "Saved: {path} is at revision {}. It shows in the user's open tab.",
        file_revision(text)
    ))
}

/// A file's revision: a hash of its bytes.
fn file_revision(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex()[..16].to_owned()
}

/// The real path of the checkout's file `path`, which must stay inside it.
fn tab_file_in(root: &std::path::Path, path: &str) -> Result<std::path::PathBuf> {
    let relative = super::side_chat::tab_file_path(path)?;
    let unreadable = |err: std::io::Error| Error::Invalid(format!("couldn't read {path}: {err}"));
    let root = root.canonicalize().map_err(unreadable)?;
    // Resolved, so a symlink can't lead out of the checkout.
    let full = root.join(relative).canonicalize().map_err(unreadable)?;
    if !full.starts_with(&root) || !full.is_file() {
        return Err(Error::Invalid(format!(
            "{path} is not a file inside the checkout"
        )));
    }
    Ok(full)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn of_saves_on_the_same_revision_only_one_goes_in() {
        let dir = std::env::temp_dir().join(format!("brigadier-tab-save-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("notes.md");
        std::fs::write(&file, "first").unwrap();
        let base = file_revision(b"first");
        let racers = 16;
        let start = std::sync::Arc::new(std::sync::Barrier::new(racers));
        let saved = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..racers)
                .map(|n| {
                    let (file, base, start) = (&file, &base, start.clone());
                    scope.spawn(move || {
                        start.wait();
                        save_checked(file, "notes.md", base, format!("edit {n}").as_bytes()).is_ok()
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .filter(|ok| *ok)
                .count()
        });
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(saved, 1, "every save but one must get a conflict");
    }

    #[test]
    fn a_save_never_overwrites_a_write_made_after_its_check() {
        let dir = std::env::temp_dir().join(format!("brigadier-tab-save-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("notes.md");
        std::fs::write(&file, "first").unwrap();
        let base = file_revision(b"first");
        // An editor or a worker saves between the check and the save going in.
        let saved = save_checked_with(&file, "notes.md", &base, b"from the chat", || {
            std::fs::write(&file, "newer").unwrap();
        });
        let held = std::fs::read_to_string(&file).unwrap();
        let left: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        let _ = std::fs::remove_dir_all(&dir);
        let err = saved.expect_err("the save must be refused").to_string();
        assert!(err.contains("changed since revision"), "{err}");
        assert!(err.contains(&file_revision(b"newer")), "{err}");
        assert_eq!(held, "newer", "the newer write must stay");
        assert_eq!(
            left,
            vec![std::ffi::OsString::from("notes.md")],
            "nothing left beside it"
        );
    }

    #[test]
    fn a_save_with_an_atomic_editor_write_meanwhile_keeps_the_editor_text() {
        let dir = std::env::temp_dir().join(format!("brigadier-tab-save-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("notes.md");
        std::fs::write(&file, "first").unwrap();
        let base = file_revision(b"first");
        // Editors often save a new file and rename it over the old one.
        let saved = save_checked_with(&file, "notes.md", &base, b"from the chat", || {
            let other = dir.join("editor.tmp");
            std::fs::write(&other, "newer").unwrap();
            std::fs::rename(&other, &file).unwrap();
        });
        let held = std::fs::read_to_string(&file).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(saved.is_err(), "the save must be refused");
        assert_eq!(held, "newer", "the editor's save must stay");
    }
}
