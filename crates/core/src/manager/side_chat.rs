//! Side chats (the "Side chat" tab, ⌥⌘S): a temporary Chat beside a conversation, for
//! questions about it that shouldn't enter its thread. Each of its turns carries the
//! conversation's latest messages; like any Chat it changes nothing. It goes when its tab
//! closes, or with the conversation.
//!
//! A tab's own chat (the chat in a split beside a page, a file or a web page) is a side chat
//! of its session too, bound to that one tab ([`TabChat`]): it is about the tab, not the
//! session's thread, and it goes without a trace when the tab closes (see [`TabChats`]).

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use super::SessionManager;
use crate::model::{
    Conversation, ConversationId, ConversationKind, Lifecycle, Setup, TabChat, TabTarget,
};
use crate::sessions::Origin;
use crate::{Error, Result};

const SIDE_CHAT_TITLE: &str = "Side chat";
const TAB_CHAT_TITLE: &str = "Chat";
/// The most of a web page's text a tab chat's turn carries.
const BROWSER_TEXT_CHARS: usize = 60_000;

/// The tab chats' state that is never stored.
#[derive(Default)]
pub(crate) struct TabChats {
    /// Whether the app has said which tab chats its tabs still have, since this start: until
    /// then none runs, since one whose tab closed while Brigadier was down must go first.
    checked: AtomicBool,
    /// What the app sent of each browser tab's page for the next turn of its chat.
    pages: Mutex<HashMap<ConversationId, BrowserPage>>,
}

/// A web page as its browser tab showed it when the user wrote.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BrowserPage {
    pub url: String,
    pub title: String,
    pub text: String,
    pub selection: String,
}

impl SessionManager {
    /// The conversation's side chat, started if it has none, on the model the conversation
    /// uses.
    pub async fn open_side_chat(
        &self,
        parent: &ConversationId,
        side_id: Option<ConversationId>,
    ) -> Result<Conversation> {
        let conversation = self.core.conversation(parent)?;
        if conversation.side_of.is_some() {
            return Err(Error::Invalid(
                "a side chat has no side chat of its own".into(),
            ));
        }
        if conversation.lifecycle == Lifecycle::Archived {
            return Err(Error::Invalid(
                "an archived conversation has no side chat".into(),
            ));
        }
        if let Some(id) = side_id.as_ref() {
            if uuid::Uuid::parse_str(&id.0).is_err() {
                return Err(Error::Invalid("side chat id must be a UUID".into()));
            }
            if let Ok(existing) = self.core.conversation(id) {
                if existing.side_of.as_ref() != Some(parent)
                    || existing.tab_chat.is_some()
                    || existing.lifecycle == Lifecycle::Archived
                {
                    return Err(Error::Invalid(
                        "this side chat does not belong to the session".into(),
                    ));
                }
                return Ok(existing);
            }
        } else if let Some(existing) = self
            .side_chats(parent)
            .into_iter()
            .find(|side| side.tab_chat.is_none())
        {
            return Ok(existing);
        }
        let model = match conversation.setup {
            Some(Setup::Session { orchestrator, .. }) => orchestrator,
            Some(Setup::Chat { model }) => model,
            None => {
                return Err(Error::Invalid(
                    "the conversation has no model yet; send it a message first".into(),
                ));
            }
        };
        self.core
            .create_conversation(
                side_id.unwrap_or_else(ConversationId::generate),
                ConversationKind::Chat,
                None,
                Some(SIDE_CHAT_TITLE.into()),
                Some(Setup::Chat { model }),
                Origin {
                    forked_from: None,
                    side_of: Some(parent.clone()),
                    tab_chat: None,
                },
            )
            .await
    }

    /// The conversation's side chats that weren't archived, its tabs' own chats too.
    pub(super) fn side_chats(&self, parent: &ConversationId) -> Vec<Conversation> {
        self.core
            .catalog()
            .conversations
            .into_iter()
            .filter(|side| {
                side.side_of.as_ref() == Some(parent) && side.lifecycle != Lifecycle::Archived
            })
            .collect()
    }

    /// Deletes a conversation's side chats: it was archived or deleted.
    pub(super) async fn delete_side_chats(&self, parent: &ConversationId) {
        for side in self.side_chats(parent) {
            // A side chat has none of its own, so this goes one level deep.
            if let Err(err) =
                Box::pin(self.delete_conversation(side.id.clone(), false, false)).await
            {
                crate::logging::private_trace!(side.tab_chat.is_some(), warn, "could not delete a side chat"; side_chat = %side.id, error = %err);
            }
        }
    }

    /// The chat of the session's tab `tab_id`, about `target`: `chat_id`'s (the id the app
    /// keeps for the tab), made on the session's model if there is none yet.
    pub async fn open_tab_chat(
        &self,
        parent: &ConversationId,
        tab_id: String,
        target: TabTarget,
        chat_id: ConversationId,
    ) -> Result<Conversation> {
        let session = self.core.conversation(parent)?;
        let Some(Setup::Session { orchestrator, .. }) = &session.setup else {
            return Err(Error::Invalid("only a session's tabs have chats".into()));
        };
        if session.lifecycle == Lifecycle::Archived || session.deleting {
            return Err(Error::Invalid(
                "an archived session's tabs have no chat".into(),
            ));
        }
        if uuid::Uuid::parse_str(&chat_id.0).is_err() {
            return Err(Error::Invalid("a tab chat's id must be a UUID".into()));
        }
        if let TabTarget::File { path } = &target {
            tab_file_path(path)?;
        }
        if let Ok(existing) = self.core.conversation(&chat_id) {
            let bound = existing.side_of.as_ref() == Some(parent)
                && existing
                    .tab_chat
                    .as_ref()
                    .is_some_and(|tab| tab.tab_id == tab_id && tab.target == target);
            if !bound || existing.deleting {
                return Err(Error::Invalid(
                    "this chat does not belong to the tab".into(),
                ));
            }
            return Ok(existing);
        }
        self.core
            .create_conversation(
                chat_id,
                ConversationKind::Chat,
                None,
                Some(TAB_CHAT_TITLE.into()),
                Some(Setup::Chat {
                    model: orchestrator.clone(),
                }),
                Origin {
                    forked_from: None,
                    side_of: Some(parent.clone()),
                    tab_chat: Some(TabChat { tab_id, target }),
                },
            )
            .await
    }

    /// The app's tabs still have these chats: every other tab chat goes (its tab closed while
    /// the app or Brigadier was down), and from now on tab chats may run.
    pub async fn keep_tab_chats(&self, keep: Vec<ConversationId>) -> Result<()> {
        let keep: HashSet<ConversationId> = keep.into_iter().collect();
        let gone: Vec<ConversationId> = self
            .core
            .catalog()
            .conversations
            .into_iter()
            .filter(|chat| chat.tab_chat.is_some() && !keep.contains(&chat.id))
            .map(|chat| chat.id)
            .collect();
        for id in gone {
            self.wipe_tab_chat(id).await;
        }
        self.tab_chats.checked.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// Page `page_id` was deleted: the chats of the tabs that showed it go.
    pub async fn page_deleted(&self, page_id: &str) {
        let gone: Vec<ConversationId> = self
            .core
            .catalog()
            .conversations
            .into_iter()
            .filter(|chat| {
                matches!(&chat.tab_chat, Some(TabChat { target: TabTarget::Page { page_id: page }, .. }) if page == page_id)
            })
            .map(|chat| chat.id)
            .collect();
        for id in gone {
            self.wipe_tab_chat(id).await;
        }
    }

    /// At launch: tab chats whose session is gone (deleted, archived, or being deleted) go.
    pub(super) async fn sweep_tab_chats(&self) {
        let catalog = self.core.catalog().conversations;
        let live: HashSet<&ConversationId> = catalog
            .iter()
            .filter(|session| session.lifecycle != Lifecycle::Archived && !session.deleting)
            .map(|session| &session.id)
            .collect();
        let gone: Vec<ConversationId> = catalog
            .iter()
            .filter(|chat| {
                chat.tab_chat.is_some()
                    && !chat
                        .side_of
                        .as_ref()
                        .is_some_and(|session| live.contains(session))
            })
            .map(|chat| chat.id.clone())
            .collect();
        for id in gone {
            self.wipe_tab_chat(id).await;
        }
    }

    /// Live drivers retain this bit after their catalog entry has gone. Unknown ids are
    /// treated as private too: a late failure must not reintroduce a deleted tab's id.
    pub(super) fn tab_chat_logs(&self, id: &ConversationId) -> bool {
        self.core
            .conversation(id)
            .map(|chat| chat.tab_chat.is_some())
            .unwrap_or_else(|_| {
                self.convs_lock()
                    .get(id)
                    .map(|live| live.private_logs)
                    .unwrap_or(true)
            })
    }

    /// Stops a tab chat if it works and removes all of it: its messages, files and records.
    async fn wipe_tab_chat(&self, id: ConversationId) {
        self.tab_chats.forget_page(&id);
        if let Err(err) = Box::pin(self.delete_conversation(id.clone(), false, false)).await {
            crate::logging::private_trace!(true, warn, "could not delete a tab chat"; tab_chat = %id, error = %err);
        }
    }

    /// A tab chat runs only once the app has said its tab is still there.
    pub(super) fn tab_chat_may_run(&self, conversation: &Conversation) -> Result<()> {
        if conversation.tab_chat.is_some() && !self.tab_chats.checked.load(Ordering::SeqCst) {
            return Err(Error::Invalid(
                "this chat starts once Brigadier's window is open".into(),
            ));
        }
        Ok(())
    }

    /// Keeps what a browser tab shows for its chat's next turn.
    pub fn set_tab_page(&self, chat: &ConversationId, page: BrowserPage) -> Result<()> {
        let record = self.core.conversation(chat)?;
        if !matches!(
            record.tab_chat,
            Some(TabChat {
                target: TabTarget::Browser,
                ..
            })
        ) {
            return Err(Error::Invalid("this chat isn't a browser tab's".into()));
        }
        self.tab_chats.pages_lock().insert(chat.clone(), page);
        Ok(())
    }

    /// What a tab chat's turn carries about its tab, and what it may do.
    pub(super) fn tab_chat_context(&self, id: &ConversationId, tab: &TabChat) -> String {
        match &tab.target {
            TabTarget::Page { page_id } => format!(
                "[This chat is beside the user's page {page_id}, in the same window. Read it with read_page and change it with write_page (page_id {page_id}) when the user asks; no other page.]"
            ),
            TabTarget::File { path } => format!(
                "[This chat is beside the file {path} of the session's checkout, in the same window. Read it with read_tab_file and change it with write_tab_file when the user asks; every other file you may only read.]"
            ),
            TabTarget::Browser => match self.tab_chats.pages_lock().get(id) {
                Some(page) => browser_context(page),
                None => "[This chat is beside a web page in Brigadier's browser; what it shows wasn't sent with this message.]".into(),
            },
        }
    }
}

impl TabChats {
    fn pages_lock(&self) -> std::sync::MutexGuard<'_, HashMap<ConversationId, BrowserPage>> {
        self.pages.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Lets go of what a browser tab sent its chat: the chat goes.
    pub(super) fn forget_page(&self, chat: &ConversationId) {
        self.pages_lock().remove(chat);
    }

    #[cfg(test)]
    pub(super) fn has_page(&self, chat: &ConversationId) -> bool {
        self.pages_lock().contains_key(chat)
    }
}

/// The web page a browser tab's chat is about, as the turn reads it.
fn browser_context(page: &BrowserPage) -> String {
    let text: String = page.text.chars().take(BROWSER_TEXT_CHARS).collect();
    let cut = if text.len() < page.text.len() {
        "\n[the page's text goes on]"
    } else {
        ""
    };
    let selection = if page.selection.trim().is_empty() {
        String::new()
    } else {
        format!("\nThe user selected: \"{}\"", page.selection.trim())
    };
    format!(
        "[This chat is beside a web page in Brigadier's browser, as it is now. You can read it but not click or type in it.]\nAddress: {}\nTitle: {}{selection}\n[page text]\n{text}{cut}\n[/page text]",
        page.url, page.title
    )
}

/// A tab file's path inside the checkout: relative, with no way out of it.
pub(crate) fn tab_file_path(path: &str) -> Result<&std::path::Path> {
    let relative = std::path::Path::new(path);
    if path.is_empty()
        || relative.is_absolute()
        || relative
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return Err(Error::Invalid(format!(
            "{path} is not a path inside the checkout"
        )));
    }
    Ok(relative)
}
