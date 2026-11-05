//! Once-only delivery to a worker's CLI. A row, or anything else kept until its CLI took it,
//! leaves its store only after delivery, so a crash in between would send it again. It goes
//! with a mark naming it; on the way back up, [`SessionManager::worker_took`] asks the CLI's own
//! record whether a turn already carries that mark.

use crate::work::Task;

use super::super::SessionManager;

/// How a once-only mark starts.
const MARK: &str = "[Worker message ";

/// The mark of what is delivered once under `id`.
pub(crate) fn once_mark(id: &str) -> String {
    format!("{MARK}{id}]")
}

/// `text` with the mark of `id` at its end: where it leaves a mention's place and what the text
/// starts with as they were. A slash command goes unmarked, since the mark would be its argument.
pub(crate) fn marked_once(text: &str, id: &str) -> String {
    if text.trim_start().starts_with('/') {
        return text.to_owned();
    }
    format!("{text}\n\n{}", once_mark(id))
}

/// A message's text as it was written, without its mark: on a line of its own after the words,
/// with what Brigadier added for the message's files, if any, after it.
pub(crate) fn without_mark(text: &str) -> String {
    let start = format!("\n\n{MARK}");
    let Some(at) = text.find(&start) else {
        return text.to_owned();
    };
    match text[at + 2..].find(']') {
        Some(len) if !text[at + 2..at + 2 + len].contains('\n') => {
            format!("{}{}", &text[..at], &text[at + 2 + len + 1..])
        }
        _ => text.to_owned(),
    }
}

impl SessionManager {
    /// Whether the worker's CLI session already took the input carrying `mark`, on any account
    /// it may have run on.
    pub(crate) async fn worker_took(&self, task: &Task, mark: &str) -> bool {
        let Some(native_id) = self.last_worker_native_id(task).await else {
            return false;
        };
        // The preferred account may have changed while the daemon was down.
        for candidate in self.runtime.account_candidates(task.route.choice.provider) {
            if let Ok(provider) = self.runtime.provider_for(&candidate.account)
                && provider.took_input(&native_id, mark).await
            {
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mark_goes_at_the_end_and_comes_off_again() {
        let marked = marked_once("Use blue.", "row-1");
        assert_eq!(marked, "Use blue.\n\n[Worker message row-1]");
        assert_eq!(without_mark(&marked), "Use blue.");
        assert_eq!(
            without_mark("Use [Worker message row-1] here"),
            "Use [Worker message row-1] here"
        );
        assert_eq!(
            without_mark(&format!("{marked}\n\nThe files: a.png")),
            "Use blue.\n\nThe files: a.png"
        );
        assert_eq!(marked_once("/compact", "row-1"), "/compact");
    }
}
