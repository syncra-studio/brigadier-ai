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
use crate::tools::{ReadPage, TabChatCall, ToolReply, WritePage, WriteTabFile};
use crate::{Error, Result};
use std::sync::Arc;

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
        "\n[page {id} \"{name}\"] Sent from this page of the user's: read it with read_page and change it with write_page (page_id {id}). Only this page, and only until the user writes again."
    )
}

impl SessionManager {
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
    async fn page_access(&self, id: &ConversationId, page_id: &str) -> Result<Arc<Pages>> {
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
                "not saved: the page changed since revision {} (it is at revision {} now). Read it again with read_page and redo your change on what it holds now.",
                base, current.revision
            ))),
        }
    }
}

impl SessionManager {
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

    /// The tab chat's file: its checkout and its path inside it.
    fn tab_file(&self, id: &ConversationId) -> Result<(std::path::PathBuf, String)> {
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
        Ok((self.checkout_dir(&session)?.into(), path))
    }

    async fn read_tab_file(&self, id: &ConversationId) -> Result<String> {
        let (root, path) = self.tab_file(id)?;
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
        let (root, path) = self.tab_file(id)?;
        super::blocking(move || {
            let full = tab_file_in(&root, &path)?;
            let now = std::fs::read(&full)
                .map_err(|err| Error::Invalid(format!("couldn't read {path}: {err}")))?;
            let current = file_revision(&now);
            if current != args.base_revision {
                return Err(Error::Invalid(format!(
                    "not saved: {path} changed since revision {} (it is at revision {current} now). Read it again with read_tab_file and redo your change on what it holds now.",
                    args.base_revision
                )));
            }
            write_beside(&full, args.text.as_bytes())
                .map_err(|err| Error::Invalid(format!("couldn't save {path}: {err}")))?;
            Ok(format!(
                "Saved: {path} is at revision {}. It shows in the user's open tab.",
                file_revision(args.text.as_bytes())
            ))
        })
        .await
    }
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

/// Replaces `path`'s bytes through a file beside it, so a reader never sees half of them. The
/// file keeps its permissions.
fn write_beside(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let temp = path.with_file_name(format!(".{name}.{}.tmp", uuid::Uuid::new_v4()));
    let written = std::fs::write(&temp, bytes).and_then(|()| {
        if let Ok(metadata) = std::fs::metadata(path) {
            std::fs::set_permissions(&temp, metadata.permissions())?;
        }
        std::fs::rename(&temp, path)
    });
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
}
