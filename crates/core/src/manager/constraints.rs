//! Standing constraints (BUILD-PLAN D1.1, research A1): the user's rules, kept in their own
//! words.
//!
//! The thread records one with `remember` (kind `constraint`), citing the user's message and, for
//! part of it, the exact span; Brigadier checks both and stores the words exactly as the user
//! wrote them, never a paraphrase. Each has a scope (the request it was said in, the session, or
//! the project) and may name the constraint it replaces: nothing is superseded unless the thread
//! says so, since a constraint wrongly dropped is a loss. An exception or a narrowing is a
//! constraint of its own, and the one it qualifies stays.
//!
//! Every rebirth briefing and every worker brief (a new worker's first message and a hand-off's)
//! carries every active constraint in scope, word for word. A very long one (a pasted spec) is
//! carried up to [`WORDS_MAX`] bytes, and the cut names the message and the byte range left out.
//!
//! A constraint is a Brain node of kind [`NodeKind::Constraint`] in the project's Brain; its key
//! holds its scope and owner ([`key`]), its body the user's words.

use brigadier_brain::{NodeFilter, NodeKind, NodeState, Origin, Provenance};

use super::brains::{brain_error, says};
use super::{SessionManager, blocking};
use crate::model::ConversationId;
use crate::tools::{ConstraintScope, Remember};
use crate::work::RequestState;
use crate::{Error, Result, now_ms};

/// A constraint's words carried in a brief, at most.
pub(crate) const WORDS_MAX: usize = 4_000;
/// The heading of a rebirth briefing's constraints section.
pub(crate) const REBIRTH_HEADING: &str =
    "[The user's standing constraints, in their own words: each holds until the user changes it]";

/// An active constraint, as briefs carry it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Constraint {
    pub id: String,
    pub scope: ConstraintScope,
    /// The user's message it was said in (`#N`, as search_transcript shows it).
    pub seq: i64,
    /// The user's words, exactly.
    pub words: String,
    /// The constraint it replaced, if any.
    pub replaces: Option<String>,
}

/// A constraint's key: `constraint:<scope>:<owner>:<message>:<words hash>`. The owner is the
/// project's (`-`), the session's id, or the session's id and the request's
/// (`<conversation>/<request>`). The same words from the same message keep one node.
pub(crate) fn key(
    scope: ConstraintScope,
    conversation: &ConversationId,
    request: &str,
    seq: i64,
    words: &str,
) -> String {
    let owner = match scope {
        ConstraintScope::Project => "-".to_owned(),
        ConstraintScope::Session => conversation.0.clone(),
        ConstraintScope::Request => format!("{}/{request}", conversation.0),
    };
    let hash = hex(&blake3::hash(words.as_bytes()).as_bytes()[..6]);
    format!("constraint:{}:{owner}:{seq}:{hash}", scope_name(scope))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn scope_name(scope: ConstraintScope) -> &'static str {
    match scope {
        ConstraintScope::Request => "request",
        ConstraintScope::Session => "session",
        ConstraintScope::Project => "project",
    }
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

/// The span of `text` that `quote` matches, as [`says`] matches it (case and runs of
/// whitespace aside), in the text's own words; `None` when it doesn't.
pub(crate) fn exact_span(text: &str, quote: &str) -> Option<String> {
    // Each text normalized as `says` does, with every normalized char's byte range in the
    // original.
    fn normalized(text: &str) -> (Vec<char>, Vec<(usize, usize)>) {
        let (mut chars, mut at) = (Vec::new(), Vec::new());
        let mut space = true;
        for (from, ch) in text.char_indices() {
            let to = from + ch.len_utf8();
            if ch.is_whitespace() {
                if !space {
                    chars.push(' ');
                    at.push((from, to));
                }
                space = true;
                continue;
            }
            space = false;
            for lower in ch.to_lowercase() {
                chars.push(lower);
                at.push((from, to));
            }
        }
        if chars.last() == Some(&' ') {
            chars.pop();
            at.pop();
        }
        (chars, at)
    }
    let quote = quote.trim_matches(['"', '\'', '\u{201c}', '\u{201d}', ' ']);
    let (wanted, _) = normalized(quote);
    let (have, at) = normalized(text);
    if wanted.is_empty() || wanted.len() > have.len() {
        return None;
    }
    let start =
        (0..=have.len() - wanted.len()).find(|&i| have[i..i + wanted.len()] == wanted[..])?;
    let (from, to) = (at[start].0, at[start + wanted.len() - 1].1);
    Some(text[from..to].to_owned())
}

/// `words` as a brief carries it: whole up to [`WORDS_MAX`] bytes, else cut with the message
/// and the byte range left out named.
fn carried(words: &str, seq: i64, id: &str) -> String {
    if words.len() <= WORDS_MAX {
        return words.to_owned();
    }
    let mut end = WORDS_MAX;
    while !words.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n[cut here: message #{seq}, bytes {end}–{} not shown; `brigadier brain search {id}` gives it whole]",
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
    let words = carried(&constraint.words, constraint.seq, &constraint.id);
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
    /// `remember` with kind `constraint`: the user's words from the message `user_message`
    /// names (the whole message, or the exact span `quote` matches), kept verbatim with its
    /// scope; `replaces` supersedes the constraint it names, and nothing else.
    pub(crate) async fn remember_constraint(
        &self,
        id: &ConversationId,
        args: Remember,
        label: String,
    ) -> Result<String> {
        let Some(reference) = args.user_message.as_deref() else {
            return Err(Error::Invalid(
                "not kept: a constraint is the user's own rule, so cite their message in `user_message` (`latest`, or the #N search_transcript shows), with `quote` for the exact part that is the rule".into(),
            ));
        };
        let said = self
            .user_words(id, reference, args.quote.as_deref())
            .await
            .map_err(|reason| Error::Invalid(format!("not kept: {reason}")))?;
        let words = match &said.quote {
            Some(quote) => exact_span(&said.words, quote).ok_or_else(|| {
                Error::Invalid(format!(
                    "not kept: the user's message #{} doesn't say \"{quote}\"",
                    said.seq
                ))
            })?,
            None => said.words.trim().to_owned(),
        };
        debug_assert!(says(&said.words, &words));
        let scope = args.scope.unwrap_or(ConstraintScope::Session);
        let project = self.project_of(id).ok_or_else(|| {
            Error::Invalid("not kept: this conversation has no project to keep it in".into())
        })?;
        let brain = self.project_brain(&project).await?.brain.clone();
        if let Some(old) = &args.replaces {
            let found = {
                let (brain, old) = (brain.clone(), old.clone());
                blocking(move || brain.node(&old).map_err(brain_error)).await?
            };
            match found {
                Some(node) if node.kind == NodeKind::Constraint => {}
                Some(node) => {
                    return Err(Error::Invalid(format!(
                        "not kept: {old} is a {:?}, not a constraint; a constraint replaces only another constraint",
                        node.kind
                    )));
                }
                None => {
                    return Err(Error::Invalid(format!(
                        "not kept: there is no constraint {old}"
                    )));
                }
            }
        }
        let node = brigadier_brain::NewNode {
            kind: NodeKind::Constraint,
            key: Some(key(scope, id, &said.id, said.seq, &words)),
            title: label,
            body: words.clone(),
            provenance: Provenance {
                origin: Origin::User,
                session_id: Some(id.0.clone()),
                task_id: None,
                job_id: None,
                worker: None,
                commit: None,
                recorded_at_ms: now_ms(),
                claim: None,
            },
            files: Vec::new(),
            expires_at_ms: None,
        };
        let replaces = args.replaces.clone();
        let node_id = blocking(move || {
            match replaces {
                Some(old) => brain.record_replacing(node, &old, "replaced by the user"),
                None => brain.record(node),
            }
            .map_err(brain_error)
        })
        .await?;
        Ok(format!(
            "Kept constraint {node_id} for {}, in the user's words from message #{}: \"{}\"{}. Every briefing and worker brief carries it verbatim while it holds.",
            match scope {
                ConstraintScope::Request => "this request",
                ConstraintScope::Session => "this session",
                ConstraintScope::Project => "every session of this project",
            },
            said.seq,
            carried(&words, said.seq, &node_id),
            args.replaces
                .map(|old| format!("; it replaces {old}"))
                .unwrap_or_default()
        ))
    }

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
    fn a_quote_is_kept_in_the_users_own_words() {
        let text = "Two things.  First, a standing rule: every new exported function\ngets a JSDoc \"Since 0.5:\" line. Reply in one line.";
        assert_eq!(
            exact_span(text, "every new exported function gets a JSDoc"),
            Some("every new exported function\ngets a JSDoc".to_owned())
        );
        assert_eq!(
            exact_span(text, "\"A STANDING RULE\""),
            Some("a standing rule".to_owned())
        );
        // Part of a word at either end, as a quote cut at punctuation is.
        assert_eq!(exact_span(text, "rule"), Some("rule".to_owned()));
        assert_eq!(
            exact_span(text, "JSDoc \"Since 0.5:\" line"),
            Some("JSDoc \"Since 0.5:\" line".to_owned())
        );
        assert_eq!(exact_span(text, "every old function"), None);
        assert_eq!(exact_span(text, "  "), None);
    }

    #[test]
    fn a_key_holds_its_scope_owner_and_message() {
        let conversation = ConversationId("conv-1".into());
        for (scope, owner) in [
            (ConstraintScope::Project, "-"),
            (ConstraintScope::Session, "conv-1"),
            (ConstraintScope::Request, "conv-1/req-9"),
        ] {
            let key = key(scope, &conversation, "req-9", 12, "never touch X");
            assert_eq!(parse_key(&key), Some((scope, owner, 12)), "{key}");
        }
        // The same words from the same message keep one key; other words get another.
        assert_eq!(
            key(ConstraintScope::Session, &conversation, "r", 3, "a"),
            key(ConstraintScope::Session, &conversation, "r", 3, "a")
        );
        assert_ne!(
            key(ConstraintScope::Session, &conversation, "r", 3, "a"),
            key(ConstraintScope::Session, &conversation, "r", 3, "b")
        );
        assert_eq!(parse_key("decision:x"), None);
    }
}
