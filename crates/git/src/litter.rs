//! Deterministic pre-commit litter guard; no model or filesystem access.
//!
//! Always exclude `*.log`, `.DS_Store`, `*.tmp`, `*.swp` and `*~`. For untracked files also
//! exclude anything under `__pycache__`, `node_modules`, `.pytest_cache` or `target` at any
//! depth, and `scratch*`, `notes*.md`, `NOTES*.md`, `TODO*.md`, `debug_*`, `tmp_*` and
//! `test_output*` unless the report lists that very file (a `NOTES.md` the task asked for).
//! Other untracked files are kept only when the report lists that file or an ancestor directory;
//! [`unreported`] names them, so a caller can hand them back to the worker instead.
//! Tracked changes are kept unless an always-excluded pattern matches. The fixed Keep/Exclude
//! API has no warning variant: callers should flag unreported tracked changes to the reviewer
//! by comparing retained changes with the report, without excluding them.

use crate::Change;

/// Whether a worker's change should be included in its candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Keep this path in the candidate.
    Keep,
    /// Leave this path out of the candidate.
    Exclude {
        /// Exact deterministic rule that excluded it.
        reason: String,
    },
}

/// Classify changes in input order. Reported entries are repo-relative file or parent directory
/// paths; separators are `/`. Only listing the file itself overrides a scratch-like name; no
/// report overrides the other exclusion rules.
pub fn classify(changes: &[Change], reported: &[String]) -> Vec<(Change, Verdict)> {
    changes.iter().map(|change| {
        let pattern = litter_pattern(change).or_else(|| {
            scratch_name(change).filter(|_| !reported.iter().any(|p| lists_file(p, &change.path)))
        });
        let verdict = if let Some(pattern) = pattern {
            Verdict::Exclude { reason: format!("matches litter pattern {pattern}") }
        } else if change.untracked && !reported.iter().any(|p| reported_contains(p, &change.path)) {
            Verdict::Exclude { reason: "untracked path is not listed in the worker report (or a reported parent directory)".into() }
        } else { Verdict::Keep };
        (change.clone(), verdict)
    }).collect()
}

/// The new files of `changes` left out only because no report names them (or a parent
/// directory): not litter, not a scratch name. Unlike litter, these can be real work, so
/// callers hand them back to the worker rather than drop them unseen.
pub fn unreported(changes: &[Change], reported: &[String]) -> Vec<String> {
    changes
        .iter()
        .filter(|change| {
            change.untracked
                && litter_pattern(change).is_none()
                && scratch_name(change).is_none()
                && !reported.iter().any(|p| reported_contains(p, &change.path))
        })
        .map(|change| change.path.clone())
        .collect()
}

fn lists_file(reported: &str, path: &str) -> bool {
    reported.strip_prefix("./").unwrap_or(reported) == path
}

fn reported_contains(reported: &str, path: &str) -> bool {
    let reported = reported
        .strip_prefix("./")
        .unwrap_or(reported)
        .trim_end_matches('/');
    reported == "."
        || (!reported.is_empty()
            && (path == reported
                || path
                    .strip_prefix(reported)
                    .is_some_and(|tail| tail.starts_with('/'))))
}

fn litter_pattern(change: &Change) -> Option<&'static str> {
    let name = change.path.rsplit('/').next().unwrap_or(&change.path);
    for (suffix, pattern) in [
        (".log", "*.log"),
        (".tmp", "*.tmp"),
        (".swp", "*.swp"),
        ("~", "*~"),
    ] {
        if name.ends_with(suffix) {
            return Some(pattern);
        }
    }
    if name == ".DS_Store" {
        return Some(".DS_Store");
    }
    if change.untracked {
        for component in change
            .path
            .split('/')
            .take(change.path.split('/').count().saturating_sub(1))
        {
            match component {
                "__pycache__" => return Some("__pycache__/"),
                "node_modules" => return Some("node_modules/"),
                ".pytest_cache" => return Some(".pytest_cache/"),
                "target" => return Some("target/"),
                _ => {}
            }
        }
    }
    None
}

/// An untracked file named like a worker's scratch notes or throwaway scripts.
fn scratch_name(change: &Change) -> Option<&'static str> {
    let name = change.path.rsplit('/').next().unwrap_or(&change.path);
    if change.untracked {
        for (prefix, pattern) in [
            ("scratch", "scratch*"),
            ("debug_", "debug_*"),
            ("tmp_", "tmp_*"),
            ("test_output", "test_output*"),
        ] {
            if name.starts_with(prefix) {
                return Some(pattern);
            }
        }
        if name.ends_with(".md") {
            for (prefix, pattern) in [
                ("notes", "notes*.md"),
                ("NOTES", "NOTES*.md"),
                ("TODO", "TODO*.md"),
            ] {
                if name.starts_with(prefix) {
                    return Some(pattern);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ChangeKind;

    fn new_file(path: &str) -> Change {
        Change {
            path: path.into(),
            kind: ChangeKind::Added,
            untracked: true,
        }
    }

    fn kept(path: &str, reported: &[&str]) -> bool {
        let reported: Vec<String> = reported.iter().map(|p| (*p).to_owned()).collect();
        classify(&[new_file(path)], &reported)[0].1 == Verdict::Keep
    }

    #[test]
    fn a_listed_file_with_a_scratch_name_is_kept() {
        assert!(kept("NOTES.md", &["NOTES.md"]));
        assert!(kept("docs/TODO.md", &["./docs/TODO.md"]));
        assert!(kept("src/debug_view.rs", &["src/debug_view.rs"]));
    }

    #[test]
    fn a_scratch_name_under_a_listed_directory_stays_out() {
        assert!(!kept("NOTES.md", &["."]));
        assert!(!kept("docs/notes-1.md", &["docs"]));
        assert!(!kept("NOTES.md", &[]));
    }

    #[test]
    fn listing_never_keeps_logs_or_build_output() {
        assert!(!kept("build.log", &["build.log"]));
        assert!(!kept(
            "node_modules/x/index.js",
            &["node_modules/x/index.js"]
        ));
        assert!(!kept("target/out.txt", &["target/out.txt"]));
    }

    #[test]
    fn only_unlisted_real_files_count_as_unreported() {
        let changes: Vec<Change> = [
            "src/discounts.ts",
            "src/discounts.test.ts",
            "src/cart.ts",
            "lib/util.ts",
            "debug.log",
            "NOTES.md",
            "node_modules/x/index.js",
            "src/debug_dump.ts",
        ]
        .into_iter()
        .map(new_file)
        .collect();
        let reported = vec!["src/cart.ts".to_owned(), "lib".to_owned()];
        assert_eq!(
            unreported(&changes, &reported),
            ["src/discounts.ts", "src/discounts.test.ts"]
        );
        // A tracked change needs no report to be kept.
        let tracked = Change {
            path: "src/app.ts".into(),
            kind: ChangeKind::Modified,
            untracked: false,
        };
        assert!(unreported(&[tracked], &[]).is_empty());
        assert!(unreported(&changes, &[".".to_owned()]).is_empty());
    }

    #[test]
    fn an_unlisted_new_file_stays_out() {
        assert!(!kept("src/new.rs", &["src/other.rs"]));
        assert!(kept("src/new.rs", &["src"]));
    }
}
