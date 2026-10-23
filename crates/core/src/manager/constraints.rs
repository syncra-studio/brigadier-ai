//! Standing constraints (BUILD-PLAN D1.1, research A1): the user's rules, kept in their own
//! words.
//!
//! Nothing records new ones any more (`remember` is gone, STRIP-DOWN-PLAN S16c): the ones a
//! project's Brain holds are still carried, in the user's words. Each has a scope (the request it
//! was said in, the session, or the project) and may name the constraint it replaced.
//!
//! Every rebirth briefing and every worker brief (a new worker's first message and a hand-off's)
//! carries every active constraint in scope, word for word. A very long one (a pasted spec) is
//! carried up to [`WORDS_MAX`] bytes, and the cut names the message and the byte range left out.
//!
//! A constraint is a Brain node of kind [`NodeKind::Constraint`] in the project's Brain; its key
//! holds its scope, owner and message (`constraint:<scope>:<owner>:<message>:<hash>`), its body
//! the user's words.

use brigadier_brain::{NodeFilter, NodeKind, NodeState};

use super::brains::brain_error;
use super::{SessionManager, blocking};
use crate::model::ConversationId;
use crate::work::RequestState;

/// A constraint's words carried in a brief, at most.
pub(crate) const WORDS_MAX: usize = 4_000;
/// The heading of a rebirth briefing's constraints section.
pub(crate) const REBIRTH_HEADING: &str =
    "[The user's standing constraints, in their own words: each holds until the user changes it]";

/// How long a constraint holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConstraintScope {
    /// While the request it was said in is open.
    Request,
    /// For this session.
    Session,
    /// For every session of this project.
    Project,
}

/// An active constraint, as briefs carry it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Constraint {
    pub id: String,
    pub scope: ConstraintScope,
    /// The user's message it was said in (`#N`).
    pub seq: i64,
    /// The user's words, exactly.
    pub words: String,
    /// The constraint it replaced, if any.
    pub replaces: Option<String>,
}

/// The scope, owner and message of a constraint's key.
fn parse_key(key: &str) -> Option<(ConstraintScope, &str, i64)> {
    let mut parts = key.split(':');
    if parts.next()? != "constraint" {
        return None;
    }
    let scope = match parts.next()? {
        "request" => ConstraintScope::Request,
        "session" => ConstraintScope::Session,
        "project" => ConstraintScope::Project,
        _ => return None,
    };
    let owner = parts.next()?;
    let seq = parts.next()?.parse().ok()?;
    Some((scope, owner, seq))
}

/// `words` as a brief carries it: whole up to [`WORDS_MAX`] bytes, else cut with the message
/// and the byte range left out named.
fn carried(words: &str, seq: i64) -> String {
    if words.len() <= WORDS_MAX {
        return words.to_owned();
    }
    let mut end = WORDS_MAX;
    while !words.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n[cut here: message #{seq}, bytes {end}–{} not shown; `brigadier transcript search` finds the rest]",
        &words[..end],
        words.len()
    )
}

/// One constraint's line: its id, scope and source, what it replaced, then the words.
fn line(constraint: &Constraint) -> String {
    let replaces = constraint
        .replaces
        .as_ref()
        .map(|old| format!(", replaces {old}"))
        .unwrap_or_default();
    let scope = match constraint.scope {
        ConstraintScope::Request => "this request",
        ConstraintScope::Session => "this session",
        ConstraintScope::Project => "every session of this project",
    };
    let words = carried(&constraint.words, constraint.seq);
    format!(
        "- {} (for {scope}; the user's message #{}{replaces}):\n  \"{}\"",
        constraint.id,
        constraint.seq,
        words.replace('\n', "\n  ")
    )
}

/// The rebirth briefing's constraints section; empty without any.
pub(crate) fn briefing_text(constraints: &[Constraint]) -> String {
    if constraints.is_empty() {
        return String::new();
    }
    let mut text = format!("{REBIRTH_HEADING}\n");
    for constraint in constraints {
        text.push_str(&line(constraint));
        text.push('\n');
    }
    text.truncate(text.trim_end().len());
    text
}

impl SessionManager {
    /// Every constraint in force for `id`: the project's, the session's, and those of its
    /// requests still open, oldest first. Empty without a project or a readable Brain.
    pub(crate) async fn active_constraints(&self, id: &ConversationId) -> Vec<Constraint> {
        let Some(project) = self.project_of(id) else {
            return Vec::new();
        };
        let brain = match self.project_brain(&project).await {
            Ok(project) => project.brain.clone(),
            Err(err) => {
                tracing::warn!(conversation = %id, error = %err, "no constraints: the project's Brain");
                return Vec::new();
            }
        };
        let nodes = blocking(move || {
            brain
                .nodes(&NodeFilter {
                    kinds: vec![NodeKind::Constraint],
                    session_id: None,
                    current_only: false,
                    text: None,
                    limit: Some(5_000),
                })
                .map_err(brain_error)
        })
        .await;
        let nodes = match nodes {
            Ok(nodes) => nodes,
            Err(err) => {
                tracing::warn!(conversation = %id, error = %err, "no constraints: the Brain");
                return Vec::new();
            }
        };
        let board = self.core.board(id).await.ok();
        let open = |request: &str| {
            board.as_ref().is_some_and(|board| {
                board.requests.get(request).is_some_and(|request| {
                    matches!(request.state, RequestState::Working | RequestState::Waiting)
                })
            })
        };
        let mut active: Vec<(i64, Constraint)> = Vec::new();
        for node in &nodes {
            if matches!(node.state, NodeState::Superseded { .. }) {
                continue;
            }
            let Some((scope, owner, seq)) = node.key.as_deref().and_then(parse_key) else {
                continue;
            };
            let holds = match scope {
                ConstraintScope::Project => true,
                ConstraintScope::Session => owner == id.0,
                ConstraintScope::Request => owner
                    .split_once('/')
                    .is_some_and(|(conversation, request)| conversation == id.0 && open(request)),
            };
            if !holds {
                continue;
            }
            let replaces = nodes
                .iter()
                .find(
                    |old| matches!(&old.state, NodeState::Superseded { by, .. } if *by == node.id),
                )
                .map(|old| old.id.clone());
            active.push((
                node.created_at_ms,
                Constraint {
                    id: node.id.clone(),
                    scope,
                    seq,
                    words: node.body.clone(),
                    replaces,
                },
            ));
        }
        active.sort_by(|a, b| (a.0, &a.1.id).cmp(&(b.0, &b.1.id)));
        active
            .into_iter()
            .map(|(_, constraint)| constraint)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_holds_its_scope_owner_and_message() {
        for (key, scope, owner) in [
            (
                "constraint:project:-:12:ab12",
                ConstraintScope::Project,
                "-",
            ),
            (
                "constraint:session:conv-1:12:ab12",
                ConstraintScope::Session,
                "conv-1",
            ),
            (
                "constraint:request:conv-1/req-9:12:ab12",
                ConstraintScope::Request,
                "conv-1/req-9",
            ),
        ] {
            assert_eq!(parse_key(key), Some((scope, owner, 12)), "{key}");
        }
        assert_eq!(parse_key("decision:x"), None);
    }
}
