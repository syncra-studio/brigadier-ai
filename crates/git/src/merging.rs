//! The Source panel's own git: merging a branch into the checkout's, resolving its conflicts
//! file by file, aborting it; switching branches, pulling and fetching. Each change takes the
//! landing lock, as a commit does, and leaves git's own refusals (local changes in the way, a
//! branch with no upstream) to the caller as errors.

use std::fs;

use crate::{
    Error, Repo, Result,
    command::{TempIndex, failure, valid_path},
    parse,
};

/// A branch the Source panel can merge from or switch to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchRef {
    /// Its short name: `main`, or `origin/main` for a remote branch.
    pub name: String,
    /// A remote-tracking branch.
    pub remote: bool,
    /// When its tip was committed, in seconds since the Unix epoch.
    pub date: i64,
    /// Its tip's author.
    pub author: String,
    /// Its tip's subject.
    pub subject: String,
}

/// A merge in progress in a checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeState {
    /// What is being merged in: a branch's short name, else its commit's short id.
    pub incoming: String,
    /// The message git prepared for the merge commit, without its comment lines.
    pub message: String,
}

/// A conflicted file's versions, for its conflict and merge editor tabs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictFile {
    /// The file as it is now, conflict markers and all; absent when it was deleted.
    pub text: Option<String>,
    /// The checkout's own version (stage 2); absent when its side deleted it.
    pub current: Option<String>,
    /// The incoming version (stage 3); absent when that side deleted it.
    pub incoming: Option<String>,
    /// Both sides merged again from their common version with every conflict marked as
    /// `<<<<<<< Current`, `||||||| Base`, `=======` and `>>>>>>> Incoming`; absent when a side
    /// deleted the file or it isn't text.
    pub merged: Option<String>,
}

impl Repo {
    /// The local branches, then the remote ones (without their `HEAD`), each newest first.
    pub fn branch_refs(&self) -> Result<Vec<BranchRef>> {
        let bytes = self.cmd(
            &[
                "for-each-ref",
                "--sort=-committerdate",
                "--format=%(refname)%00%(committerdate:unix)%00%(authorname)%00%(subject)%00",
                "refs/heads/",
                "refs/remotes/",
            ],
            true,
        )?;
        let mut fields = parse::fields(&bytes);
        let mut refs = Vec::new();
        while let Some(name) = fields.next() {
            let name = name.strip_prefix(b"\n").unwrap_or(name);
            if name.is_empty() {
                continue;
            }
            let mut next = || {
                fields
                    .next()
                    .ok_or_else(|| Error::Parse("missing branch field".into()))
                    .and_then(|field| parse::text(field).map(str::to_owned))
            };
            let (date, author, subject) = (next()?, next()?, next()?);
            let name = parse::text(name)?;
            let (short, remote) = match name.strip_prefix("refs/heads/") {
                Some(short) => (short, false),
                None => match name.strip_prefix("refs/remotes/") {
                    Some(short) if !short.ends_with("/HEAD") => (short, true),
                    _ => continue,
                },
            };
            refs.push(BranchRef {
                name: short.to_owned(),
                remote,
                date: date.parse().unwrap_or(0),
                author,
                subject,
            });
        }
        refs.sort_by_key(|branch| branch.remote);
        Ok(refs)
    }

    /// The merge in progress, if any.
    pub fn merging(&self) -> Result<Option<MergeState>> {
        let dir = self.git_dir()?;
        let head = match fs::read(dir.join("MERGE_HEAD")) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let oid = parse::line(head.split(|&b| b == b'\n').next().unwrap_or_default())?;
        let names = self.cmd(
            &[
                "for-each-ref",
                "--format=%(refname:short)",
                &format!("--points-at={oid}"),
                "refs/heads/",
                "refs/remotes/",
            ],
            true,
        )?;
        let incoming = String::from_utf8_lossy(&names)
            .lines()
            .find(|name| !name.ends_with("/HEAD"))
            .map_or_else(|| oid.chars().take(7).collect(), str::to_owned);
        let message = fs::read(dir.join("MERGE_MSG")).unwrap_or_default();
        let message = String::from_utf8_lossy(&message)
            .lines()
            .filter(|line| !line.starts_with('#'))
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_owned();
        Ok(Some(MergeState { incoming, message }))
    }

    /// Merges `branch` (a local or remote branch's short name) into the checked-out one, as
    /// `git merge` does: a fast-forward or merge commit when it is clean, otherwise a merge in
    /// progress with its conflicts marked in the files. Returns whether it stopped on
    /// conflicts.
    pub fn merge_branch(&self, branch: &str) -> Result<bool> {
        let lock = self.landing_lock();
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        self.refuse_busy()?;
        let full = self.branch_ref(branch)?;
        // The short name, which git writes in the merge message and the conflict markers,
        // unless git would read it as something else (a tag of the same name).
        let picked = self.run(
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                "--symbolic-full-name",
                branch,
            ],
            true,
        )?;
        let name = if picked.status.success() && parse::line(&picked.stdout)? == full {
            branch
        } else {
            full.as_str()
        };
        // Hooks run as they would for the user's own `git merge`.
        let args = ["merge", "--no-edit", "--no-autostash", name];
        let out = self.run(&args, false)?;
        self.stopped_on_conflicts(&args, &out)
    }

    /// After a merge or pull: false when it went through, true when it stopped on conflicts,
    /// and its error otherwise (a hook's refusal leaves a merge in progress with none).
    fn stopped_on_conflicts(&self, args: &[&str], out: &std::process::Output) -> Result<bool> {
        if out.status.success() {
            return Ok(false);
        }
        if self.merging()?.is_some() && !self.cmd(&["ls-files", "--unmerged"], true)?.is_empty() {
            return Ok(true);
        }
        Err(failure(args, out))
    }

    /// Aborts the merge in progress: the branch and files go back to how they were before it.
    pub fn abort_merge(&self) -> Result<()> {
        let lock = self.landing_lock();
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        if self.merging()?.is_none() {
            return Err(Error::Invalid("there is no merge to abort".into()));
        }
        self.cmd(&["merge", "--abort"], false)?;
        Ok(())
    }

    /// Switches the checkout to `branch`. A remote branch switches to the local branch of its
    /// name, made to track it when there is none yet.
    pub fn switch_branch(&self, branch: &str) -> Result<()> {
        let lock = self.landing_lock();
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        self.refuse_busy()?;
        let full = self.branch_ref(branch)?;
        let local = match full.strip_prefix("refs/remotes/") {
            None => branch.to_owned(),
            Some(_) => {
                let local = branch
                    .split_once('/')
                    .map(|(_, rest)| rest)
                    .ok_or_else(|| Error::Invalid(format!("no branch named {branch}")))?;
                self.validate_branch(local)?;
                if self.branch_tip(local)?.is_none() {
                    self.cmd(&["switch", "--quiet", "--track", &full], false)?;
                    return Ok(());
                }
                local.to_owned()
            }
        };
        self.validate_branch(&local)?;
        self.cmd(&["switch", "--quiet", "--no-guess", &local], false)?;
        Ok(())
    }

    /// Pulls the checked-out branch from its upstream as `git pull` does, the user's own pull
    /// settings and all. Returns whether it stopped on merge conflicts.
    pub fn pull(&self) -> Result<bool> {
        let lock = self.landing_lock();
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        self.refuse_busy()?;
        let args = ["pull", "--no-edit", "--no-autostash"];
        let out = self.run(&args, false)?;
        self.stopped_on_conflicts(&args, &out)
    }

    /// Fetches from the branch's remote (`origin` when it has none). Touches no checkout.
    pub fn fetch(&self) -> Result<()> {
        self.cmd(&["fetch", "--quiet"], false)?;
        Ok(())
    }

    /// A conflicted file's versions.
    pub fn conflict_file(&self, path: &str) -> Result<ConflictFile> {
        let text = match self.read_plain_file(path)? {
            Some(bytes) => Some(String::from_utf8(bytes).map_err(|_| not_text(path))?),
            None => None,
        };
        let stage = |n: u8| -> Result<Option<Vec<u8>>> {
            let spec = format!(":{n}:{path}");
            let out = self.run(&["cat-file", "blob", &spec], true)?;
            Ok(out.status.success().then_some(out.stdout))
        };
        let (base, current, incoming) = (stage(1)?, stage(2)?, stage(3)?);
        let merged = match (&current, &incoming) {
            (Some(current), Some(incoming)) => {
                self.marked_file(base.as_deref().unwrap_or_default(), current, incoming)?
            }
            _ => None,
        };
        let text_of =
            |bytes: Option<Vec<u8>>| bytes.and_then(|bytes| String::from_utf8(bytes).ok());
        Ok(ConflictFile {
            text,
            current: text_of(current),
            incoming: text_of(incoming),
            merged,
        })
    }

    /// Writes the user's resolution of a conflicted file, only while the file is as they last
    /// read it (`expected`), and with `stage` marks it resolved.
    pub fn resolve_conflict(
        &self,
        path: &str,
        expected: Option<&str>,
        text: &str,
        stage: bool,
    ) -> Result<()> {
        valid_path(path)?;
        let lock = self.landing_lock();
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        let unmerged = self.cmd(&["ls-files", "--unmerged", "-z", "--", path], true)?;
        if unmerged.is_empty() {
            return Err(Error::Invalid(format!(
                "{path} has no conflicts to resolve"
            )));
        }
        let full = self.root.join(path);
        let now = self.read_plain_file(path)?;
        if now.as_deref() != expected.map(str::as_bytes) {
            return Err(Error::Invalid(format!(
                "{path} changed since it was opened; look at it again"
            )));
        }
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&full, text)?;
        if stage {
            self.cmd(&["add", "--", path], false)?;
        }
        Ok(())
    }

    /// A checkout file's bytes (`None`: it isn't there), read only when neither it nor a folder
    /// on its way is a link, so nothing outside the checkout is read or then written.
    fn read_plain_file(&self, path: &str) -> Result<Option<Vec<u8>>> {
        valid_path(path)?;
        let mut at = self.root.clone();
        for part in path.split('/') {
            at.push(part);
            match fs::symlink_metadata(&at) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    return Err(Error::Invalid(format!(
                        "{path} is a link; resolve it with git in a terminal"
                    )));
                }
                Ok(meta) if at != self.root.join(path) && !meta.is_dir() => {
                    return Err(Error::Invalid(format!("{path} isn't in a folder")));
                }
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(e) => return Err(e.into()),
            }
        }
        Ok(Some(fs::read(&at)?))
    }

    /// Refuses while the checkout is in the middle of another git operation.
    fn refuse_busy(&self) -> Result<()> {
        match self.busy()? {
            Some(what) => Err(Error::Invalid(format!("the checkout is busy ({what})"))),
            None => Ok(()),
        }
    }

    /// The full ref of a local or remote branch's short name.
    fn branch_ref(&self, branch: &str) -> Result<String> {
        if branch.starts_with('-') || branch.contains('\0') {
            return Err(Error::Invalid(format!("no branch named {branch}")));
        }
        for full in [
            format!("refs/heads/{branch}"),
            format!("refs/remotes/{branch}"),
        ] {
            let out = self.run(&["rev-parse", "--verify", "--quiet", &full], true)?;
            if out.status.success() {
                return Ok(full);
            }
        }
        Err(Error::Invalid(format!("no branch named {branch}")))
    }

    /// `current` and `incoming` merged from `base` with every conflict marked, base included;
    /// `None` when git can't merge them as text.
    fn marked_file(&self, base: &[u8], current: &[u8], incoming: &[u8]) -> Result<Option<String>> {
        // A private folder of its own, removed when dropped.
        let scratch = TempIndex::new()?;
        let [base_file, current_file, incoming_file] =
            ["base", "current", "incoming"].map(|name| scratch.path.with_file_name(name));
        fs::write(&base_file, base)?;
        fs::write(&current_file, current)?;
        fs::write(&incoming_file, incoming)?;
        let args = [
            "merge-file".as_ref(),
            "-p".as_ref(),
            "--diff3".as_ref(),
            "-L".as_ref(),
            "Current".as_ref(),
            "-L".as_ref(),
            "Base".as_ref(),
            "-L".as_ref(),
            "Incoming".as_ref(),
            current_file.as_os_str(),
            base_file.as_os_str(),
            incoming_file.as_os_str(),
        ];
        let out = self.git.run(None, &args, true, &[], None)?;
        // The exit code counts the conflicts; a negative one (shown as 255) is a failure, a
        // binary file among them.
        Ok(match out.status.code() {
            Some(0..=127) => String::from_utf8(out.stdout).ok(),
            _ => None,
        })
    }
}

fn not_text(path: &str) -> Error {
    Error::Invalid(format!("{path} isn't a text file"))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::repo::tests::test_git;

    const BASE: &str = "a\nb\nc\n";

    /// A repository on `main` whose `b` line `main` and `feature` each changed their own way.
    fn conflicting(name: &str) -> (crate::repo::tests::Temp, crate::Repo, std::path::PathBuf) {
        let (dir, git) = test_git(name);
        let root = dir.join("repo");
        assert!(git.init(&root).expect("git init").is_none());
        let repo = git.open(&root).expect("the repository");
        repo.cmd(&["checkout", "-q", "-b", "main"], false).unwrap();
        fs::write(root.join("f.txt"), BASE).unwrap();
        fs::write(root.join("other.txt"), "x\n").unwrap();
        repo.commit_changes("Base", true).unwrap();
        repo.cmd(&["checkout", "-q", "-b", "feature"], false)
            .unwrap();
        fs::write(root.join("f.txt"), "a\nfeature\nc\n").unwrap();
        fs::write(root.join("other.txt"), "y\n").unwrap();
        repo.commit_changes("Feature", true).unwrap();
        repo.cmd(&["checkout", "-q", "main"], false).unwrap();
        fs::write(root.join("f.txt"), "a\nmain\nc\n").unwrap();
        repo.commit_changes("Main", true).unwrap();
        (dir, repo, root)
    }

    #[test]
    fn a_conflicting_merge_is_resolved_staged_and_committed_from_the_source_panel() {
        let (_dir, repo, root) = conflicting("merge-resolve");
        assert!(
            repo.merge_branch("feature").expect("the merge"),
            "it stops on conflicts"
        );
        let merging = repo.merging().unwrap().expect("a merge in progress");
        assert_eq!(merging.incoming, "feature");
        assert!(
            merging.message.starts_with("Merge branch 'feature'"),
            "{}",
            merging.message
        );
        let (changes, _) = repo.source_status().unwrap();
        assert!(
            changes
                .changes
                .iter()
                .any(|e| e.path == "f.txt" && e.kind == crate::SourceKind::Conflicted)
        );
        assert!(
            changes.staged.iter().any(|e| e.path == "other.txt"),
            "the clean part is staged"
        );

        // Committing waits for the conflict to be resolved.
        assert!(repo.commit_changes("Too soon", false).is_err());
        let file = repo.conflict_file("f.txt").unwrap();
        let text = file.text.clone().expect("the marked file");
        assert!(
            text.contains("<<<<<<< HEAD") && text.contains(">>>>>>> feature"),
            "{text}"
        );
        assert_eq!(file.current.as_deref(), Some("a\nmain\nc\n"));
        assert_eq!(file.incoming.as_deref(), Some("a\nfeature\nc\n"));
        assert_eq!(
            file.merged.as_deref(),
            Some(
                "a\n<<<<<<< Current\nmain\n||||||| Base\nb\n=======\nfeature\n>>>>>>> Incoming\nc\n"
            )
        );

        // A resolution written over a file that changed since it was read is refused.
        assert!(
            repo.resolve_conflict("f.txt", Some("stale"), "a\nboth\nc\n", true)
                .is_err()
        );
        assert_eq!(fs::read_to_string(root.join("f.txt")).unwrap(), text);
        repo.resolve_conflict("f.txt", Some(&text), "a\nmain\nfeature\nc\n", false)
            .unwrap();
        // Unstaged, it is still conflicted; staging it (the panel's +) resolves it.
        assert!(repo.commit_changes("Still too soon", false).is_err());
        repo.stage(Some(&["f.txt".to_owned()]))
            .expect("staging during a merge");
        let commit = repo
            .commit_changes(&merging.message, false)
            .expect("the merge commit");
        assert!(repo.merging().unwrap().is_none());
        let parents = repo
            .read_only(&["rev-list", "--parents", "-n1", &commit.0])
            .unwrap();
        assert_eq!(
            parents.split_whitespace().count(),
            3,
            "a merge commit: {parents}"
        );
        assert!(
            repo.resolve_conflict("f.txt", None, "x", true).is_err(),
            "nothing left to resolve"
        );
    }

    #[test]
    fn a_merge_is_aborted_and_a_clean_one_commits_at_once() {
        let (_dir, repo, root) = conflicting("merge-abort");
        let before = repo.resolve("HEAD").unwrap();
        assert!(repo.merge_branch("feature").unwrap());
        assert!(repo.merge_branch("feature").is_err(), "one merge at a time");
        repo.abort_merge().unwrap();
        assert!(repo.merging().unwrap().is_none());
        assert_eq!(repo.resolve("HEAD").unwrap(), before);
        assert_eq!(
            fs::read_to_string(root.join("f.txt")).unwrap(),
            "a\nmain\nc\n"
        );
        assert!(repo.abort_merge().is_err(), "no merge to abort");

        repo.cmd(&["checkout", "-q", "-b", "side"], false).unwrap();
        fs::write(root.join("new.txt"), "n\n").unwrap();
        repo.commit_changes("New", true).unwrap();
        repo.switch_branch("main").unwrap();
        assert!(!repo.merge_branch("side").unwrap(), "clean");
        assert!(repo.merging().unwrap().is_none());
        assert!(root.join("new.txt").exists());
        assert!(repo.merge_branch("--help").is_err());
        assert!(repo.merge_branch("nowhere").is_err());
    }

    #[test]
    fn branches_list_local_then_remote_and_a_remote_one_is_switched_to_as_its_own() {
        let (dir, repo, root) = conflicting("merge-branches");
        let remote = dir.join("remote.git");
        repo.cmd(
            &[
                "clone",
                "-q",
                "--bare",
                root.to_str().unwrap(),
                remote.to_str().unwrap(),
            ],
            false,
        )
        .unwrap();
        repo.cmd(
            &["remote", "add", "origin", remote.to_str().unwrap()],
            false,
        )
        .unwrap();
        repo.fetch().unwrap();
        repo.cmd(&["branch", "-D", "feature"], false).unwrap();
        let refs = repo.branch_refs().unwrap();
        let names: Vec<(&str, bool)> = refs.iter().map(|b| (b.name.as_str(), b.remote)).collect();
        assert!(names.contains(&("main", false)));
        assert!(names.contains(&("origin/feature", true)));
        assert!(!names.iter().any(|(name, _)| name.ends_with("/HEAD")));
        let first_remote = names.iter().position(|(_, remote)| *remote).unwrap();
        assert!(
            names[first_remote..].iter().all(|(_, remote)| *remote),
            "{names:?}"
        );
        let main = refs.iter().find(|b| b.name == "main").unwrap();
        assert_eq!(
            (main.author.as_str(), main.subject.as_str()),
            ("Test", "Main")
        );
        assert!(main.date > 0);

        repo.switch_branch("origin/feature").unwrap();
        assert_eq!(
            repo.state().unwrap().current_branch.as_deref(),
            Some("feature")
        );
        assert_eq!(
            repo.remote_state("feature").unwrap().upstream.as_deref(),
            Some("origin/feature")
        );
        repo.switch_branch("main").unwrap();
        repo.switch_branch("origin/feature").unwrap();
        assert_eq!(
            repo.state().unwrap().current_branch.as_deref(),
            Some("feature")
        );
        assert!(!repo.pull().unwrap(), "up to date");
    }

    #[test]
    fn amend_rewrites_the_last_commit_keeping_its_message_when_blank() {
        let (_dir, repo, root) = conflicting("merge-amend");
        let before = repo.resolve("HEAD").unwrap();
        fs::write(root.join("late.txt"), "l\n").unwrap();
        repo.stage(None).unwrap();
        let amended = repo.commit_with("", false, true).unwrap();
        assert_ne!(amended, before);
        assert_eq!(
            repo.read_only(&["log", "-1", "--format=%s"])
                .unwrap()
                .trim(),
            "Main"
        );
        assert_eq!(
            repo.read_only(&["rev-list", "--count", "HEAD"])
                .unwrap()
                .trim(),
            "3"
        );
    }

    #[test]
    fn a_branch_named_like_a_tag_merges_the_branch() {
        let (_dir, repo, root) = conflicting("merge-tag");
        repo.cmd(&["tag", "side", "HEAD~1"], false).unwrap();
        repo.cmd(&["checkout", "-q", "-b", "side"], false).unwrap();
        fs::write(root.join("new.txt"), "n\n").unwrap();
        repo.commit_changes("New", true).unwrap();
        repo.switch_branch("main").unwrap();
        assert!(!repo.merge_branch("side").unwrap());
        assert!(
            root.join("new.txt").exists(),
            "the branch's commit came in, not the tag's"
        );
    }

    #[test]
    fn a_hook_refusing_a_clean_merge_is_an_error_not_a_conflict() {
        let (_dir, repo, root) = conflicting("merge-hook");
        repo.cmd(&["checkout", "-q", "-b", "side", "HEAD~1"], false)
            .unwrap();
        fs::write(root.join("new.txt"), "n\n").unwrap();
        repo.commit_changes("New", true).unwrap();
        repo.switch_branch("main").unwrap();
        let hooks = root.join(".git/hooks");
        fs::create_dir_all(&hooks).unwrap();
        let hook = hooks.join("pre-merge-commit");
        fs::write(&hook, "#!/bin/sh\necho no merges today >&2\nexit 1\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let error = repo.merge_branch("side").unwrap_err().to_string();
        assert!(error.contains("no merges today"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn a_conflicted_link_is_neither_read_through_nor_written_through() {
        let (dir, repo, root) = conflicting("merge-link");
        assert!(repo.merge_branch("feature").unwrap());
        let outside = dir.join("outside.txt");
        fs::write(&outside, "keep\n").unwrap();
        fs::remove_file(root.join("f.txt")).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("f.txt")).unwrap();
        assert!(repo.conflict_file("f.txt").is_err());
        assert!(
            repo.resolve_conflict("f.txt", Some("keep\n"), "gone\n", true)
                .is_err()
        );
        assert_eq!(fs::read_to_string(&outside).unwrap(), "keep\n");
    }
}
