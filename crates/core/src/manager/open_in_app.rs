//! `open_in_app` (Q90): the thread or a Chat shows the user something in the session's tabs, as
//! the user's own click would open it. The daemon checks the target is there and names it; the
//! window opens it, in front only when the user asked to see it and is on the session, else
//! behind the tab they are on, marked new. A tab of the user's that already shows it, behind
//! theirs, is left as it is.

use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use super::SessionManager;
use crate::app_call::{AppAnswer, AppOp, DiffScope, OpenTarget, OutputOf};
use crate::model::{CardId, Setup, TaskId};
use crate::tools::{OpenDiffScope, OpenInApp, OpenInAppTarget};
use crate::work::ApprovalSubject;
use crate::{ConversationId, Error, Result};

/// How long the window may take to open it.
const OPEN_WAIT: Duration = Duration::from_secs(10);

impl SessionManager {
    /// `open_in_app`'s answer, in plain words.
    pub(crate) async fn open_in_app(&self, id: &ConversationId, args: OpenInApp) -> Result<String> {
        let (target, what) = self.open_target(id, args.target).await?;
        let label = self.browser_label(id, None).await;
        let op = AppOp::Open {
            target,
            show: args.show,
        };
        match self.app.call(id, None, &label, op, OPEN_WAIT).await {
            AppAnswer::Opened { tab, front, kept } => {
                Ok(opened_words(&what, tab.as_deref(), front, kept, args.show))
            }
            AppAnswer::Failed { message } => Err(Error::Invalid(message)),
            _ => Err(Error::Invalid(
                "Brigadier's window answered something else.".into(),
            )),
        }
    }

    /// The target as the window opens it, checked against the session, and what it is in words.
    async fn open_target(
        &self,
        id: &ConversationId,
        target: OpenInAppTarget,
    ) -> Result<(OpenTarget, String)> {
        let conversation = self.core.conversation(id)?;
        let session = matches!(conversation.setup, Some(Setup::Session { .. }));
        let tabs_only = |what: &str| {
            Error::Invalid(format!(
                "A Chat has no tabs, so {what} can't be opened here; tell the user in words."
            ))
        };
        Ok(match target {
            OpenInAppTarget::File { path, line } => {
                if !session {
                    return Err(tabs_only("a file"));
                }
                let root = self.checkout_dir(id)?;
                let relative = checkout_path(Path::new(&root), &path)?;
                let words = match line {
                    Some(line) => format!("{relative} at line {line}"),
                    None => relative.clone(),
                };
                (
                    OpenTarget::File {
                        path: relative,
                        line: line.filter(|line| *line > 0),
                    },
                    words,
                )
            }
            OpenInAppTarget::Diff { scope } => {
                if !session {
                    return Err(tabs_only("a diff"));
                }
                let (scope, words) = match scope {
                    OpenDiffScope::LastTurn => (DiffScope::LastTurn, "the last turn's changes"),
                    OpenDiffScope::Branch => (DiffScope::Branch, "the branch's changes"),
                    OpenDiffScope::Uncommitted => {
                        (DiffScope::Uncommitted, "the uncommitted changes")
                    }
                };
                (OpenTarget::Diff { scope }, words.to_owned())
            }
            OpenInAppTarget::Progress => {
                if !session {
                    return Err(tabs_only("the Progress tab"));
                }
                (OpenTarget::Progress, "the Progress tab".to_owned())
            }
            OpenInAppTarget::Output { id: wanted } => {
                if !session {
                    return Err(tabs_only("an Output tab"));
                }
                let board = self.core.board(id).await?;
                let wanted = wanted.trim().to_owned();
                let Some(preview) = board.previews.get(&wanted) else {
                    return Err(Error::Invalid(format!(
                        "there is no preview {wanted} in this session"
                    )));
                };
                let name = if preview.name.is_empty() {
                    &preview.command
                } else {
                    &preview.name
                };
                let (source, title) = (OutputOf::Preview, first_up(name));
                let words = format!("the output of {title}");
                (
                    OpenTarget::Output {
                        source,
                        id: wanted,
                        title,
                    },
                    words,
                )
            }
            OpenInAppTarget::Url { url } => {
                let url = url.trim().to_owned();
                if !(url.starts_with("http://") || url.starts_with("https://")) {
                    return Err(Error::Invalid(format!(
                        "{url} isn't a web address (http or https)"
                    )));
                }
                if !session {
                    return Err(tabs_only("a Browser tab"));
                }
                if !self.core.settings().browser_enabled {
                    return Err(Error::Invalid(
                        "The built-in browser is off in Settings › Browser, so the page can't open in Brigadier; give the user the address instead.".into(),
                    ));
                }
                let words = url.clone();
                (OpenTarget::Url { url }, words)
            }
            OpenInAppTarget::Worker { task } => {
                let board = self.core.board(id).await?;
                let task = TaskId(task.trim().to_owned());
                let Some(found) = board.tasks.get(&task) else {
                    return Err(Error::Invalid(format!(
                        "there is no worker {} in this session",
                        task.0
                    )));
                };
                let words = format!("{}'s panel", found.title);
                (OpenTarget::Worker { task }, words)
            }
            OpenInAppTarget::Plan { id: wanted } => {
                if !session {
                    return Err(tabs_only("a plan"));
                }
                let board = self.core.board(id).await?;
                let card = CardId(wanted.trim().to_owned());
                if let Some(plan) = board.plans.get(&card) {
                    if plan
                        .body
                        .as_deref()
                        .is_none_or(|body| body.trim().is_empty())
                    {
                        return Err(Error::Invalid(format!(
                            "plan {} has no text to read; open the Progress tab instead",
                            card.0
                        )));
                    }
                    let words = format!("the plan \"{}\"", plan.title);
                    (
                        OpenTarget::Plan {
                            id: card.0,
                            outline: false,
                            title: plan.title.clone(),
                        },
                        words,
                    )
                } else if let Some(ApprovalSubject::Outline { title, .. }) =
                    board.approvals.get(&card).map(|approval| &approval.subject)
                {
                    let words = format!("the outline \"{title}\"");
                    (
                        OpenTarget::Plan {
                            id: card.0,
                            outline: true,
                            title: title.clone(),
                        },
                        words,
                    )
                } else {
                    return Err(Error::Invalid(format!(
                        "there is no plan or outline {} in this session",
                        card.0
                    )));
                }
            }
            OpenInAppTarget::Page { id: page_id } => {
                if !session {
                    return Err(tabs_only("a page"));
                }
                let page_id = page_id.trim().to_owned();
                let pages = self.page_access(id, &page_id).await?;
                let wanted = page_id.clone();
                let page = super::blocking(move || {
                    pages
                        .read(&wanted)
                        .map_err(|err| Error::Invalid(err.to_string()))
                })
                .await?;
                let title = page.content.title;
                let words = if title.is_empty() {
                    "the page".to_owned()
                } else {
                    format!("the page \"{title}\"")
                };
                (OpenTarget::Page { id: page_id, title }, words)
            }
        })
    }
}

/// `path` (relative to the checkout, or absolute inside it) as the checkout's own relative path,
/// when it names a file there.
fn checkout_path(root: &Path, path: &str) -> Result<String> {
    let path = path.trim();
    let given = Path::new(path);
    let joined = if given.is_absolute() {
        given.to_path_buf()
    } else {
        root.join(given)
    };
    let outside = || Error::Invalid(format!("{path} isn't a file in this session's checkout"));
    if joined
        .components()
        .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(outside());
    }
    let root = root.canonicalize().map_err(|_| outside())?;
    let file: PathBuf = joined.canonicalize().map_err(|_| outside())?;
    if !file.is_file() {
        return Err(outside());
    }
    let relative = file.strip_prefix(&root).map_err(|_| outside())?;
    if relative.components().next() == Some(Component::Normal(".git".as_ref())) {
        return Err(outside());
    }
    Ok(relative.to_string_lossy().replace('\\', "/"))
}

/// What happened, as the agent hears it.
fn opened_words(what: &str, tab: Option<&str>, front: bool, kept: bool, show: bool) -> String {
    let tab = tab.map(|tab| format!(" (tab {tab})")).unwrap_or_default();
    if kept {
        return format!(
            "{} is already open{tab}, in the user's own tab or the one in front; it was left as it is.",
            first_up(what)
        );
    }
    if front {
        return format!("Opened {what}{tab} in front.");
    }
    let why = if show {
        " The user isn't on this session now, so it waits there for them."
    } else {
        ""
    };
    format!("Opened {what}{tab} behind the user's tab, marked new.{why}")
}

/// `words` with its first letter up: "Dev app", "Core tests".
fn first_up(words: &str) -> String {
    let mut chars = words.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_is_the_checkouts_own_or_refused() {
        struct Scratch(PathBuf);
        impl Drop for Scratch {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let dir = Scratch(std::env::temp_dir().join(format!(
            "brigadier-open-in-app-{}-{}",
            std::process::id(),
            crate::now_ms()
        )));
        let root = dir.0.as_path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".git/config"), "").unwrap();
        assert_eq!(checkout_path(root, "src/main.rs").unwrap(), "src/main.rs");
        let absolute = root.canonicalize().unwrap().join("src/main.rs");
        assert_eq!(
            checkout_path(root, absolute.to_str().unwrap()).unwrap(),
            "src/main.rs"
        );
        for refused in [
            "src/missing.rs",
            "src",
            "../outside.rs",
            "/etc/hosts",
            ".git/config",
        ] {
            assert!(checkout_path(root, refused).is_err(), "{refused}");
        }
    }

    #[test]
    fn the_answer_says_where_it_opened() {
        assert_eq!(
            opened_words("src/main.rs", Some("file:src/main.rs"), true, false, true),
            "Opened src/main.rs (tab file:src/main.rs) in front."
        );
        assert_eq!(
            opened_words(
                "the Progress tab",
                Some("panel:reviews"),
                false,
                false,
                false
            ),
            "Opened the Progress tab (tab panel:reviews) behind the user's tab, marked new."
        );
        assert!(
            opened_words("the Progress tab", None, false, false, true)
                .ends_with("The user isn't on this session now, so it waits there for them.")
        );
        assert!(
            opened_words("the branch's changes", Some("review"), false, true, false).starts_with(
                "The branch's changes is already open (tab review), in the user's own tab"
            )
        );
    }
}
