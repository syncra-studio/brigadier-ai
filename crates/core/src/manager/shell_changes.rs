//! What a thread's shell command changed, told by the files it names, read just before it runs
//! and once it ended (#87). Claude's `Bash` is read from its tool hook (`PreToolUse`, then
//! `PostToolUse`), which the CLI waits for; Brigadier's own `run` around the process it starts.
//! A request's record owns a command's change only from this evidence, and only where it
//! starts from the file as the record last knew it and the file still holds what the command
//! left; everything else a command changed stays the user's to keep (request_changes.rs).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use brigadier_providers::{FileChange, FileChangeKind, ItemStatus, ProviderEvent};

use super::request_changes::Blob;
use super::{SessionManager, blocking, git_error};
use crate::model::ConversationId;
use crate::tools::Role;
use crate::work::{OrchestratorStepKind, ThreadItemKind};

/// The most files of one command read.
const MOST_FILES: usize = 64;
/// The most calls a session keeps read and not yet reported (a call the CLI never reports
/// again, refused say, ages out).
const MOST_CALLS: usize = 64;
/// The largest file read for a command: a bigger one has no evidence (the call waits for the
/// read), so what the command did to it stays the user's to keep.
const MOST_BYTES: u64 = 16 * 1024 * 1024;
/// What a command's file row keeps of each diff, as a CLI's edit keeps of its own.
const DIFF_CLIP: usize = 8 * 1024;

/// Which call a capture belongs to: a CLI's shell call by its id; Brigadier's `run` by its
/// command, in the order they ran (its tool call's id never reaches the tool).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ShellKey {
    Call(String),
    Run(String),
}

struct Capture {
    key: ShellKey,
    /// Which read this is, of all: two runs of the same command apart.
    call: u64,
    before: BTreeMap<String, Option<Blob>>,
    after: Option<BTreeMap<String, Option<Blob>>>,
    /// When it ended, in the order calls end: the same command's reports come in that order.
    ended: Option<u64>,
}

/// One command's change by its captures: each file it names that it changed, before → after.
pub(super) struct ShellEvidence {
    /// The command's item, which its file rows follow.
    pub(super) item: String,
    pub(super) changes: Vec<(String, Option<Blob>, Option<Blob>)>,
}

#[derive(Default)]
pub(super) struct ShellCaptures {
    calls: std::sync::Mutex<HashMap<ConversationId, Vec<Capture>>>,
    /// Counts the reads started and the calls ended.
    count: std::sync::atomic::AtomicU64,
}

/// The unfinished capture of `key`: call `call`'s, else the oldest.
fn unfinished<'a>(
    calls: &'a mut [Capture],
    key: &ShellKey,
    call: Option<u64>,
) -> Option<&'a mut Capture> {
    calls.iter_mut().find(|capture| {
        capture.key == *key
            && capture.after.is_none()
            && call.is_none_or(|call| capture.call == call)
    })
}

impl ShellCaptures {
    fn next(&self) -> u64 {
        self.count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    fn start(
        &self,
        id: &ConversationId,
        key: ShellKey,
        before: BTreeMap<String, Option<Blob>>,
    ) -> u64 {
        let call = self.next();
        let mut all = self.calls.lock().unwrap();
        let calls = all.entry(id.clone()).or_default();
        if calls.len() == MOST_CALLS {
            calls.remove(0);
        }
        calls.push(Capture {
            key,
            call,
            before,
            after: None,
            ended: None,
        });
        call
    }

    /// The files the call's start read: the same are read at its end.
    fn started(
        &self,
        id: &ConversationId,
        key: &ShellKey,
        call: Option<u64>,
    ) -> Option<Vec<String>> {
        let mut all = self.calls.lock().unwrap();
        unfinished(all.get_mut(id)?, key, call)
            .map(|capture| capture.before.keys().cloned().collect())
    }

    fn end(
        &self,
        id: &ConversationId,
        key: &ShellKey,
        call: Option<u64>,
        after: BTreeMap<String, Option<Blob>>,
    ) {
        let ended = self.next();
        let mut all = self.calls.lock().unwrap();
        if let Some(capture) = all
            .get_mut(id)
            .and_then(|calls| unfinished(calls, key, call))
        {
            capture.after = Some(after);
            capture.ended = Some(ended);
        }
    }

    /// The evidence of the reported call `key` (item `item`), taken: none for a call read at
    /// only one end (one that went on in the background).
    pub(super) fn take(
        &self,
        id: &ConversationId,
        key: &ShellKey,
        item: &str,
    ) -> Option<ShellEvidence> {
        let mut all = self.calls.lock().unwrap();
        let calls = all.get_mut(id)?;
        let at = calls
            .iter()
            .enumerate()
            .filter(|(_, capture)| capture.key == *key)
            .filter_map(|(at, capture)| Some((capture.ended?, at)))
            .min()
            .map(|(_, at)| at)
            .or_else(|| calls.iter().position(|capture| capture.key == *key))?;
        let capture = calls.remove(at);
        if calls.is_empty() {
            all.remove(id);
        }
        let after = capture.after?;
        // A file not read at its end leaves the command without evidence.
        let mut changes = Vec::new();
        for (path, before) in capture.before {
            let after = after.get(&path)?.clone();
            if after != before {
                changes.push((path, before, after));
            }
        }
        (!changes.is_empty()).then(|| ShellEvidence {
            item: item.to_owned(),
            changes,
        })
    }

    /// A turn ended: none of its calls is reported any more.
    pub(super) fn clear(&self, id: &ConversationId) {
        self.calls.lock().unwrap().remove(id);
    }
}

/// A command's words as the shell reads them: a quoted span or an escaped space stays in its
/// word (`mv 'old name.txt' new.txt`), and the shell's operators end one; and whether it has a
/// subshell (`(cd sub)`), whose folder changes end with it. None when the shell makes names of
/// its own from it (a glob, a variable, a brace list, a command's output, or `xargs`): which
/// files the command touched can't be told then.
fn shell_words(command: &str) -> Option<(Vec<String>, bool)> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut subshell = false;
    let mut chars = command.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => word.extend(chars.by_ref().take_while(|&c| c != '\'')),
            '"' => {
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '$' | '`' => return None,
                        // In double quotes a backslash escapes only these; else it stays.
                        '\\' => match chars.next() {
                            Some(c @ ('"' | '\\' | '\n' | '$' | '`')) => word.push(c),
                            Some(c) => {
                                word.push('\\');
                                word.push(c);
                            }
                            None => word.push('\\'),
                        },
                        c => word.push(c),
                    }
                }
            }
            '\\' => word.extend(chars.next()),
            '~' if word.is_empty() => return None,
            '*' | '?' | '[' | '{' | '}' | '$' | '`' => return None,
            c if c.is_whitespace() => {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            }
            // An operator ends a word, and shows as an empty one (`cd;` goes nowhere named).
            c if ";|&<>()".contains(c) => {
                subshell |= c == '(' || c == ')';
                words.push(std::mem::take(&mut word));
            }
            c => word.push(c),
        }
    }
    words.push(word);
    // A shell the command starts on a script of its own, which this doesn't read.
    let nested = words
        .windows(2)
        .any(|pair| matches!(pair[0].as_str(), "sh" | "bash" | "zsh" | "dash") && pair[1] == "-c");
    let unread = nested
        || words
            .iter()
            .any(|word| matches!(word.as_str(), "xargs" | "popd" | "eval"));
    (!unread).then_some((words, subshell))
}

/// The checkout's files a command names, relative to its root `root`: each word of the command
/// (as the shell reads it, and split at spaces, quotes and the shell's operators too, for the
/// names a script inside it writes), read from `cwd` or a folder the command goes to
/// (`cd <dir> && …`), that is a file in the checkout, or a new file's name in one of its
/// folders (`echo x > new.md`). Folders are left out. None when a word names a file there
/// that can't be read for the command, which Undo then couldn't put back (one outside the
/// checkout or under `.git`, one named like an option), when the shell makes names for it
/// (`mv u*.txt all.txt`), when where it goes can't be told, or when it names too many: what
/// it changed then stays the user's to keep.
pub(super) fn named_files(command: &str, cwd: &Path, root: &Path) -> Vec<String> {
    let Ok(root) = root.canonicalize() else {
        return Vec::new();
    };
    let Some((words, subshell)) = shell_words(command) else {
        return Vec::new();
    };
    // Each word is read from the folder the command is in by then: a `cd` must go to one,
    // else where its names lead is unknown, and inside a subshell it can't be followed.
    let mut dir = cwd.to_owned();
    let mut found = BTreeSet::new();
    let mut words = words.into_iter();
    while let Some(word) = words.next() {
        if matches!(word.as_str(), "cd" | "pushd") {
            let to = match words.next() {
                Some(to) if !to.is_empty() && !subshell => dir.join(to),
                _ => return Vec::new(),
            };
            if !to.is_dir() {
                return Vec::new();
            }
            dir = to;
            continue;
        }
        // The word, and its parts split at spaces, quotes and the shell's operators, for the
        // names a script inside it writes.
        let parts = word
            .split(|c: char| c.is_whitespace() || "'\"`;|&<>(){}[]=,$".contains(c))
            .filter(|part| *part != word)
            .map(str::to_owned)
            .collect::<Vec<_>>();
        for word in std::iter::once(word).chain(parts) {
            if word.is_empty() || word.contains("://") {
                continue;
            }
            let path = dir.join(&word);
            let there = std::fs::symlink_metadata(&path);
            // A folder it names (a move of one, say) holds files it doesn't name.
            if there.as_ref().is_ok_and(|metadata| metadata.is_dir()) {
                if matches!(word.as_str(), "." | "..") || path == root {
                    continue;
                }
                return Vec::new();
            }
            let named = (!word.starts_with('-') && word.len() <= 1024)
                .then(|| checkout_file(&word, &dir, &root))
                .flatten();
            match named {
                Some(relative) => {
                    found.insert(relative);
                }
                // A device (`/dev/null`) is no file Undo would put back.
                None if there.is_ok_and(|metadata| metadata.is_file() || metadata.is_symlink()) => {
                    return Vec::new();
                }
                None => {}
            }
            if found.len() > MOST_FILES {
                return Vec::new();
            }
        }
    }
    found.into_iter().collect()
}

/// `word` read from `dir` as a file of the checkout at `root`, relative to it.
fn checkout_file(word: &str, dir: &Path, root: &Path) -> Option<String> {
    let path = dir.join(word);
    let parent = path.parent()?.canonicalize().ok()?;
    let path = parent.join(path.file_name()?);
    let relative = path
        .strip_prefix(root)
        .ok()
        .and_then(Path::to_str)
        .filter(|relative| !relative.is_empty())?;
    if relative == ".git" || relative.starts_with(".git/") {
        return None;
    }
    if std::fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.is_dir()) {
        return None;
    }
    Some(relative.to_owned())
}

impl SessionManager {
    /// What a Claude thread's tool hook sends for a `Bash` call (`brigadierd hook
    /// post-tool-use`): before it runs (`ended` false) and once it ended. The files its command
    /// names are read before the hook's answer lets the call go on. A grant that isn't a live
    /// hook grant reads nothing.
    pub async fn hook_shell(
        &self,
        grant: &str,
        ended: bool,
        call: &str,
        command: &str,
        cwd: Option<&str>,
    ) {
        let Some(Role::OutputHook { conversation_id }) = self.grants.resolve(grant) else {
            return;
        };
        self.capture_shell(
            &conversation_id,
            ShellKey::Call(call.to_owned()),
            command,
            cwd.map(Path::new),
            ended.then_some(None),
        )
        .await;
    }

    /// Reads the files the command of `key` names in its session's checkout: at its start,
    /// which says which call it is, or (`ended`) the same files once that call (else the
    /// oldest of `key` unfinished) ended.
    pub(super) async fn capture_shell(
        &self,
        id: &ConversationId,
        key: ShellKey,
        command: &str,
        cwd: Option<&Path>,
        ended: Option<Option<u64>>,
    ) -> Option<u64> {
        let (checkout, _) = self.changes_checkout(id)?;
        let shells = &self.change_records.shells;
        let named = match ended {
            Some(call) => Some(shells.started(id, &key, call)?),
            None => None,
        };
        let (git, command, cwd) = (
            self.git.clone(),
            command.to_owned(),
            cwd.map_or_else(|| checkout.clone(), Path::to_owned),
        );
        // Every file named, or none: a source left unread (too big, or not a file now) could
        // be where a file the command made came from.
        let read = blocking(move || {
            let repo = git.open(&checkout).map_err(git_error)?;
            let named = named.unwrap_or_else(|| named_files(&command, &cwd, repo.root()));
            let mut files = BTreeMap::new();
            for path in named {
                if std::fs::symlink_metadata(repo.root().join(&path))
                    .is_ok_and(|metadata| metadata.len() > MOST_BYTES)
                {
                    return Ok(BTreeMap::new());
                }
                match repo.checkout_blob(&path) {
                    Ok(blob) => {
                        files.insert(path, blob.map(Blob::from));
                    }
                    // A word that only looked like a new file's name.
                    Err(_) if std::fs::symlink_metadata(repo.root().join(&path)).is_err() => {}
                    Err(err) => return Err(git_error(err)),
                }
            }
            Ok(files)
        })
        .await;
        let files = read.unwrap_or_else(|err| {
            tracing::debug!(conversation = %id, error = %err, "could not read a command's files");
            BTreeMap::new()
        });
        match ended {
            Some(call) => {
                shells.end(id, &key, call, files);
                None
            }
            None if files.is_empty() => None,
            None => Some(shells.start(id, key, files)),
        }
    }
}

impl SessionManager {
    /// The files a shell command changed that its request's record owns, as a file-changes
    /// row after the command's ("Edited notes.md +1 −0"), each opening to its diff, with the
    /// session's secrets hidden as in any of the thread's rows.
    pub(super) async fn shell_edit_row(
        &self,
        id: &ConversationId,
        request: &str,
        item: &str,
        files: Vec<(PathBuf, FileChangeKind, String)>,
    ) {
        let Ok(conv) = self.conv(id) else {
            return;
        };
        let Some(cli) = conv.live_cli().await else {
            return;
        };
        let mut changes = Vec::new();
        for (path, kind, diff) in files {
            let diff = self.hide_secrets(id, diff.into_bytes()).await;
            let diff = String::from_utf8_lossy(&diff);
            changes.push(FileChange::counted(
                path.to_string_lossy().into_owned(),
                kind,
                (!diff.is_empty()).then_some(&*diff),
                DIFF_CLIP,
            ));
        }
        let item_id = format!("{item}/edits");
        let event = ProviderEvent::FileChanges {
            item_id: item_id.clone(),
            changes: changes.clone(),
            status: ItemStatus::Completed,
        };
        let log_seq = self.log_provider_at(id, cli.provider, event).await;
        let turn = conv.items().turn();
        let detail = changes.first().map(|first| match changes.len() {
            1 => first.path.clone(),
            n => format!("{} and {} more", first.path, n - 1),
        });
        self.orchestrator_step_in(
            id,
            Some(request.to_owned()),
            OrchestratorStepKind::Tool {
                item_id,
                name: "apply_patch".into(),
                detail,
                status: ItemStatus::Completed,
                stopped: false,
                through_position: 0,
                ended_at_ms: Some(crate::now_ms()),
                exit: None,
                item: Some(ThreadItemKind::Files),
                files: Some(super::thread_items::file_stats(&changes)),
                output_bytes: None,
                full_output: None,
                log_seq,
                turn,
            },
        )
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A command names the checkout's files it writes, reads or makes, from its folder; flags,
    /// folders, `.git`, URLs and places outside are left out.
    #[test]
    fn a_command_names_the_checkouts_files() {
        let root = std::env::temp_dir().join(format!(
            "brigadier-core-named-files-{}",
            uuid::Uuid::now_v7()
        ));
        std::fs::create_dir_all(root.join("src/.git")).unwrap();
        let outside = root.with_extension("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("x.md"), "x").unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join("notes.md"), "x").unwrap();
        std::fs::write(root.join("src/lib.rs"), "x").unwrap();
        let named = |command: &str, cwd: &Path| named_files(command, cwd, &root);
        let names = |command: &str, cwd: &Path, path: &str| {
            named(command, cwd).iter().any(|named| named == path)
        };
        assert!(names("echo x >> notes.md", &root, "notes.md"));
        let perl = "perl -pi -e 's/a/b/' src/lib.rs notes.md";
        assert!(names(perl, &root, "src/lib.rs") && names(perl, &root, "notes.md"));
        let heredoc = "python3 - <<'EOF'\nopen(\"new.txt\", \"w\").write('x')\nEOF";
        assert!(
            names(heredoc, &root, "new.txt"),
            "a new file in a folder that is there"
        );
        assert!(names(
            "sed -i '' s/a/b/ lib.rs",
            &root.join("src"),
            "src/lib.rs"
        ));
        let absolute = format!("cat {}", root.join("notes.md").display());
        assert!(names(&absolute, Path::new("/"), "notes.md"));
        let elsewhere = format!("cd {} && echo x >> lib.rs", root.join("src").display());
        assert!(names(&elsewhere, Path::new("/"), "src/lib.rs"));
        let left_out = named("ls ../outside/x https://x.dev/a --flag .", &root);
        assert_eq!(left_out, ["ls"], "only a word that could be a new file");
        std::fs::write(root.join("src/notes.md"), "x").unwrap();
        let within = named("cd src && echo x >> notes.md", &root);
        assert!(within.contains(&"src/notes.md".to_owned()), "{within:?}");
        assert!(
            !within.contains(&"notes.md".to_owned()),
            "read from the folder it went to"
        );
        std::fs::write(root.join(".git/notes"), "x").unwrap();
        std::fs::write(root.join("-dash.md"), "x").unwrap();
        std::fs::create_dir_all(root.join("src/other")).unwrap();
        std::fs::write(root.join("a\\ b.md"), "x").unwrap();
        for command in [
            "mv n*.md all.md",
            "mv \"$SRC\" notes.md",
            "cp notes.{md,txt} .",
            "ls | xargs -I x mv x notes.md",
            "mv `cat list` notes.md",
            "mv ~/x.md notes.md",
            "cd; mv x.md notes.md",
            "cd nowhere && mv x.md notes.md",
            "mv .git/notes notes.md",
            "mv -- -dash.md notes.md",
            "(cd src); cd other && mv x.md notes.md",
            "mv src old-src",
            "sh -c 'mv x.md notes.md'",
            "eval mv x.md notes.md",
        ] {
            assert_eq!(named(command, &root), [] as [String; 0], "{command}");
        }
        assert!(names("echo '$x *' >> notes.md", &root, "notes.md"));
        let away = format!("mv {} notes.md", outside.join("x.md").display());
        assert_eq!(named(&away, &root), [] as [String; 0]);
        let script = format!(
            "python3 -c 'import shutil; shutil.move(\"{}\", \"notes.md\")'",
            outside.join("x.md").display()
        );
        assert_eq!(named(&script, &root), [] as [String; 0]);
        let spaced = format!("cd  {} && mv x.md notes.md", outside.display());
        assert_eq!(named(&spaced, &root), [] as [String; 0]);
        assert!(names("echo x > /dev/null; cat notes.md", &root, "notes.md"));
        assert!(names("mv \"a\\ b.md\" notes.md", &root, "a\\ b.md"));
        std::fs::write(root.join("cash$notes.md"), "x").unwrap();
        assert!(names("mv \"cash\\$notes.md\" n.md", &root, "cash$notes.md"));
        std::fs::write(root.join("old name.txt"), "x").unwrap();
        for command in [
            "mv 'old name.txt' new.txt",
            "mv \"old name.txt\" new.txt",
            "mv old\\ name.txt new.txt",
        ] {
            let named = named(command, &root);
            assert!(
                named.iter().any(|n| n == "old name.txt"),
                "{command}: {named:?}"
            );
            assert!(named.iter().any(|n| n == "new.txt"), "{command}: {named:?}");
        }
        std::fs::remove_dir_all(&root).unwrap();
        std::fs::remove_dir_all(&outside).unwrap();
    }

    /// Two runs of the same command, ended out of order: each end reads its own call's files,
    /// and the reports, which come in the order the calls ended, take them in that order.
    #[test]
    fn the_same_command_run_twice_keeps_each_calls_reads() {
        let id = ConversationId("c".into());
        let key = ShellKey::Run("make".into());
        let blob = |oid: &str| {
            Some(Blob::from(brigadier_git::FileBlob {
                oid: oid.into(),
                mode: "100644".into(),
            }))
        };
        let reads = |path: &str, blob: Option<Blob>| BTreeMap::from([(path.to_owned(), blob)]);
        let shells = ShellCaptures::default();
        let first = shells.start(&id, key.clone(), reads("a", blob("1")));
        let second = shells.start(&id, key.clone(), reads("b", blob("1")));
        assert_eq!(shells.started(&id, &key, Some(second)).unwrap(), ["b"]);
        shells.end(&id, &key, Some(second), reads("b", blob("2")));
        shells.end(&id, &key, Some(first), reads("a", blob("3")));
        let taken = shells.take(&id, &key, "one").unwrap();
        assert_eq!(taken.changes, [("b".into(), blob("1"), blob("2"))]);
        let taken = shells.take(&id, &key, "two").unwrap();
        assert_eq!(taken.changes, [("a".into(), blob("1"), blob("3"))]);
    }
}
