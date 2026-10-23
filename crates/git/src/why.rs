//! Which turn wrote a line (`brigadier why`, STRIP-DOWN-PLAN S39): a blame over the retained
//! turn trees of a checkout's sessions ([`crate::snapshot`]), newest first, with git's own
//! blame for what no turn wrote. Read-only: no object, ref or index is written.

use std::collections::{BTreeSet, HashMap};

use crate::command::{check, valid_oid, valid_path};
use crate::{Error, Oid, Repo, Result, parse};

/// A turn of a session in the checkout, as its snapshot refs keep it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhyTurn {
    /// The session's id, its ref namespace.
    pub session: String,
    /// The turn's number.
    pub turn: u64,
}

/// What wrote a line, and the line as the file has it now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhyLine {
    /// The line's text now, without its end of line.
    pub text: String,
    /// Who wrote it.
    pub origin: Origin,
}

/// Who wrote a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// A turn's own diff added the line's current content.
    Turn(TurnOrigin),
    /// No turn wrote it: git's history did (the commit `git blame` names), or nothing yet
    /// (`None`: an edit outside any turn, not committed).
    Outside(Option<BlameCommit>),
}

/// The turn that wrote a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnOrigin {
    /// The turn.
    pub turn: WhyTurn,
    /// Where the line was when the turn ended: a repo-relative path and a 1-based line.
    pub path: String,
    /// See [`Self::path`].
    pub line: usize,
    /// Later turns that only moved the line, newest first.
    pub moved_by: Vec<WhyTurn>,
    /// The commit made during the turn that holds the line, when it was committed by the
    /// turn's end.
    pub commit: Option<TurnCommit>,
}

/// A commit made during a turn that holds the line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnCommit {
    /// The commit `git blame` names at the turn's end.
    pub commit: BlameCommit,
    /// The merge, made during the turn, that brought the commit in from another branch.
    pub merge: Option<Oid>,
    /// That branch: its name, else its tip's id.
    pub branch: Option<String>,
}

/// A commit as `git blame` names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlameCommit {
    /// Its id.
    pub id: Oid,
    /// Its author's name.
    pub author: String,
    /// Its author time, ms since the epoch.
    pub at_ms: i64,
    /// Its subject line.
    pub summary: String,
}

/// Lines no longer than this many (after the shared start and end) are diffed; beyond, no
/// line of the middle is taken as kept.
const MAX_EDITS: usize = 4000;

impl crate::Git {
    /// The canonical top level of the checkout that `path` (a file or a folder) is in;
    /// `None` outside any.
    pub fn checkout_of(&self, path: &std::path::Path) -> Result<Option<std::path::PathBuf>> {
        let folder = if path.is_dir() {
            path
        } else {
            path.parent().unwrap_or(path)
        };
        let out = self.run(
            Some(folder),
            &["rev-parse", "--show-toplevel"],
            true,
            &[],
            None,
        )?;
        if !out.status.success() {
            return Ok(None);
        }
        let top = parse::line(&out.stdout)?;
        Ok(Some(std::fs::canonicalize(top)?))
    }
}

impl Repo {
    /// Which of `turns` (newest first) wrote line `line` (1-based) of the checkout's file
    /// `path` as it is now, following the line back through each later turn and the gaps
    /// between them; a line a later turn only moved (the same text deleted elsewhere in that
    /// turn) is followed to where it came from.
    pub fn why_line(&self, path: &str, line: usize, turns: &[WhyTurn]) -> Result<WhyLine> {
        valid_path(path)?;
        let now = std::fs::read(self.root.join(path))
            .map_err(|err| Error::Invalid(format!("cannot read {path}: {err}")))?;
        let lines = split(&now);
        if line == 0 || line > lines.len() {
            return Err(Error::Invalid(format!(
                "{path} has {} lines; there is no line {line}",
                lines.len()
            )));
        }
        let text = String::from_utf8_lossy(lines[line - 1]).into_owned();
        let refs = self.snapshot_refs()?;
        let mut reader = Blobs::new(self);
        let mut place = Place {
            path: path.to_owned(),
            line: line - 1,
            content: now,
            blob: None,
        };
        let mut moved_by = Vec::new();
        for turn in turns {
            let (Some(start), Some(end)) = (
                refs.get(&ref_name(&turn.session, turn.turn, false)),
                refs.get(&ref_name(&turn.session, turn.turn, true)),
            ) else {
                continue;
            };
            // The gap after the turn: from its end to what came next.
            let Some(end_blob) = reader.id(end, &place.path)? else {
                return self.outside(path, line, text);
            };
            if place.blob.as_ref() != Some(&end_blob) {
                let content = reader.read(&end_blob)?;
                match line_map(&split(&content), &split(&place.content))[place.line] {
                    Some(at) => place.line = at,
                    None => return self.outside(path, line, text),
                }
                place.content = content;
                place.blob = Some(end_blob.clone());
            }
            // The turn itself: from its start to its end.
            let start_path = match reader.id(start, &place.path)? {
                Some(_) => place.path.clone(),
                None => self
                    .renamed_from(start, end, &place.path)?
                    .unwrap_or_else(|| place.path.clone()),
            };
            let start_blob = reader.id(start, &start_path)?;
            if start_blob.as_ref() == Some(&end_blob) {
                place.path = start_path;
                continue;
            }
            let before = match &start_blob {
                Some(blob) => reader.read(blob)?,
                None => Vec::new(),
            };
            if let Some(at) = line_map(&split(&before), &split(&place.content))[place.line] {
                place = Place {
                    path: start_path,
                    line: at,
                    content: before,
                    blob: start_blob,
                };
                continue;
            }
            let added = split(&place.content)[place.line].to_vec();
            if let Some(from) = self.moved_from(&mut reader, start, end, &place, &added)? {
                moved_by.push(turn.clone());
                place = from;
                continue;
            }
            let commit = self.turn_commit(turn, &refs, &place)?;
            return Ok(WhyLine {
                text,
                origin: Origin::Turn(TurnOrigin {
                    turn: turn.clone(),
                    path: place.path,
                    line: place.line + 1,
                    moved_by,
                    commit,
                }),
            });
        }
        self.outside(path, line, text)
    }

    /// Every session and turn with both a start and an end tree, in no order.
    pub fn snapshot_turns(&self) -> Result<Vec<WhyTurn>> {
        let refs = self.snapshot_refs()?;
        let mut turns: BTreeSet<(String, u64)> = BTreeSet::new();
        for name in refs.keys() {
            let Some(rest) = name.strip_prefix("refs/brigadier/s/") else {
                continue;
            };
            let Some((session, turn)) = rest.split_once("/turn-") else {
                continue;
            };
            let Some(number) = turn.strip_suffix("-start") else {
                continue;
            };
            let Ok(number) = number.parse::<u64>() else {
                continue;
            };
            if refs.contains_key(&ref_name(session, number, true)) {
                turns.insert((session.to_owned(), number));
            }
        }
        Ok(turns
            .into_iter()
            .map(|(session, turn)| WhyTurn { session, turn })
            .collect())
    }

    /// Every snapshot ref (`refs/brigadier/s/…`) and what it points at, in one call.
    fn snapshot_refs(&self) -> Result<HashMap<String, Oid>> {
        let out = self.cmd(
            &[
                "for-each-ref",
                "--format=%(objectname) %(refname)",
                "refs/brigadier/s/",
            ],
            true,
        )?;
        Ok(parse::text(&out)?
            .lines()
            .filter_map(|line| line.split_once(' '))
            .map(|(id, name)| (name.to_owned(), Oid(id.to_owned())))
            .collect())
    }

    /// The path the turn renamed to `path`, when it did.
    fn renamed_from(&self, start: &Oid, end: &Oid, path: &str) -> Result<Option<String>> {
        let out = self.cmd(
            &[
                "diff",
                "--no-ext-diff",
                "--name-status",
                "-z",
                "--find-renames",
                &start.0,
                &end.0,
                "--",
            ],
            true,
        )?;
        let fields: Vec<&[u8]> = parse::fields(&out).collect();
        let mut at = 0;
        while at < fields.len() {
            let status = fields[at];
            if status.first() == Some(&b'R') || status.first() == Some(&b'C') {
                if at + 2 < fields.len() && fields[at + 2] == path.as_bytes() {
                    return Ok(Some(String::from_utf8_lossy(fields[at + 1]).into_owned()));
                }
                at += 3;
            } else {
                at += 2;
            }
        }
        Ok(None)
    }

    /// Where in the turn's start tree the line the turn added at `place` came from, when the
    /// turn deleted the same text somewhere (this file first, then the others it changed).
    fn moved_from(
        &self,
        reader: &mut Blobs<'_>,
        start: &Oid,
        end: &Oid,
        place: &Place,
        added: &[u8],
    ) -> Result<Option<Place>> {
        let wanted = trim(added);
        if wanted.iter().filter(|b| b.is_ascii_alphanumeric()).count() < 3 {
            return Ok(None);
        }
        let mut paths = vec![place.path.clone()];
        let (_, changed) = self.diff_scope(start, end)?;
        paths.extend(changed.into_iter().filter(|path| *path != place.path));
        for path in paths {
            let Some(blob) = reader.id(start, &path)? else {
                continue;
            };
            let before = reader.read(&blob)?;
            let after = match reader.id(end, &path)? {
                Some(blob) => reader.read(&blob)?,
                None => Vec::new(),
            };
            let old = split(&before);
            let kept: BTreeSet<usize> = line_map(&old, &split(&after))
                .into_iter()
                .flatten()
                .collect();
            let found = old
                .iter()
                .enumerate()
                .find(|(at, line)| !kept.contains(at) && trim(line) == wanted);
            if let Some((at, _)) = found {
                return Ok(Some(Place {
                    path,
                    line: at,
                    content: before,
                    blob: Some(blob),
                }));
            }
        }
        Ok(None)
    }

    /// The commit made during `turn` that holds the line at `place` (the turn's end), when
    /// the turn committed it; with the merge and branch that brought it, when one did.
    fn turn_commit(
        &self,
        turn: &WhyTurn,
        refs: &HashMap<String, Oid>,
        place: &Place,
    ) -> Result<Option<TurnCommit>> {
        let head = |end| refs.get(&format!("{}-head", ref_name(&turn.session, turn.turn, end)));
        let (Some(first), Some(last)) = (head(false), head(true)) else {
            return Ok(None);
        };
        if first == last {
            return Ok(None);
        }
        let Some(committed) = self.file_at(last, &place.path)? else {
            return Ok(None);
        };
        let Some(line) = line_map(&split(&committed), &split(&place.content))[place.line] else {
            return Ok(None);
        };
        let Some(commit) = self.blame(Some(last), &place.path, line + 1)? else {
            return Ok(None);
        };
        if self.ancestor(&commit.id, first)? {
            return Ok(None);
        }
        let range = format!("{}..{}", first.0, last.0);
        let own = self.cmd(&["rev-list", "--first-parent", &range], true)?;
        if parse::text(&own)?.lines().any(|id| id == commit.id.0) {
            return Ok(Some(TurnCommit {
                commit,
                merge: None,
                branch: None,
            }));
        }
        let merges = self.cmd(
            &[
                "rev-list",
                "--first-parent",
                "--merges",
                "--parents",
                &range,
            ],
            true,
        )?;
        for line in parse::text(&merges)?.lines() {
            let ids: Vec<&str> = line.split_whitespace().collect();
            for tip in ids.iter().skip(2) {
                let tip = Oid((*tip).to_owned());
                if !self.ancestor(&commit.id, &tip)? {
                    continue;
                }
                let names = self.cmd(
                    &[
                        "for-each-ref",
                        "--format=%(refname:short)",
                        &format!("--points-at={}", tip.0),
                        "refs/heads/",
                    ],
                    true,
                )?;
                let branch = parse::text(&names)?
                    .lines()
                    .next()
                    .map(str::to_owned)
                    .or_else(|| self.merged_branch(ids[0]));
                return Ok(Some(TurnCommit {
                    commit,
                    merge: Some(Oid(ids[0].to_owned())),
                    branch,
                }));
            }
        }
        Ok(Some(TurnCommit {
            commit,
            merge: None,
            branch: None,
        }))
    }

    /// The branch a merge commit's subject names (`Merge branch 'x'`), when the branch is gone.
    fn merged_branch(&self, merge: &str) -> Option<String> {
        let subject = self.cmd(&["log", "-1", "--format=%s", merge], true).ok()?;
        let subject = parse::text(&subject).ok()?;
        let rest = subject.strip_prefix("Merge branch '")?;
        rest.split_once('\'').map(|(name, _)| name.to_owned())
    }

    /// No turn wrote the line: what `git blame` says of it in the checkout as it is now.
    fn outside(&self, path: &str, line: usize, text: String) -> Result<WhyLine> {
        Ok(WhyLine {
            text,
            origin: Origin::Outside(self.blame(None, path, line)?),
        })
    }

    /// `git blame` of one line at `rev` (the working file when `None`); `None` when the line
    /// is not committed, or git tracks no such file.
    pub fn blame(&self, rev: Option<&Oid>, path: &str, line: usize) -> Result<Option<BlameCommit>> {
        valid_path(path)?;
        let range = format!("{line},+1");
        let mut args = vec!["blame", "--porcelain", "-L", range.as_str()];
        if let Some(rev) = rev {
            valid_oid(rev)?;
            args.push(rev.0.as_str());
        }
        args.extend(["--", path]);
        let out = self.run(&args, true)?;
        if !out.status.success() {
            // An untracked file has no history to blame.
            if self.tracks_under(path)? {
                check(&args, out)?;
            }
            return Ok(None);
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let mut lines = text.lines();
        let Some(id) = lines
            .next()
            .and_then(|first| first.split_whitespace().next())
        else {
            return Err(Error::Parse("empty git blame output".into()));
        };
        if id.bytes().all(|b| b == b'0') {
            return Ok(None);
        }
        let mut commit = BlameCommit {
            id: Oid(id.to_owned()),
            author: String::new(),
            at_ms: 0,
            summary: String::new(),
        };
        for line in lines {
            if let Some(author) = line.strip_prefix("author ") {
                commit.author = author.to_owned();
            } else if let Some(time) = line.strip_prefix("author-time ") {
                commit.at_ms = time.trim().parse::<i64>().unwrap_or(0) * 1000;
            } else if let Some(summary) = line.strip_prefix("summary ") {
                commit.summary = summary.to_owned();
            } else if line.starts_with('\t') {
                break;
            }
        }
        Ok(Some(commit))
    }
}

/// Where the walk holds the line: in which file, at which line of which content.
struct Place {
    path: String,
    /// 0-based.
    line: usize,
    content: Vec<u8>,
    /// The content's blob, when it came from a tree.
    blob: Option<Oid>,
}

/// Blob ids by tree and path, and blob contents, each read once.
struct Blobs<'a> {
    repo: &'a Repo,
    ids: HashMap<(Oid, String), Option<Oid>>,
    contents: HashMap<Oid, Vec<u8>>,
}

impl<'a> Blobs<'a> {
    fn new(repo: &'a Repo) -> Self {
        Self {
            repo,
            ids: HashMap::new(),
            contents: HashMap::new(),
        }
    }

    /// The blob at `path` in `tree`; `None` when there is no such file.
    fn id(&mut self, tree: &Oid, path: &str) -> Result<Option<Oid>> {
        let key = (tree.clone(), path.to_owned());
        if let Some(id) = self.ids.get(&key) {
            return Ok(id.clone());
        }
        valid_oid(tree)?;
        let listed = self
            .repo
            .cmd(&["ls-tree", "-z", &tree.0, "--", path], true)?;
        let entry = listed.split(|b| *b == 0).next().unwrap_or_default();
        let head = entry.split(|b| *b == b'\t').next().unwrap_or_default();
        let fields: Vec<&[u8]> = head.split(|b| *b == b' ').collect();
        let id = match fields.as_slice() {
            [_, b"blob", object] => Some(Oid(String::from_utf8_lossy(object).into_owned())),
            _ => None,
        };
        self.ids.insert(key, id.clone());
        Ok(id)
    }

    fn read(&mut self, blob: &Oid) -> Result<Vec<u8>> {
        if let Some(content) = self.contents.get(blob) {
            return Ok(content.clone());
        }
        valid_oid(blob)?;
        let content = self.repo.cmd(&["cat-file", "blob", &blob.0], true)?;
        self.contents.insert(blob.clone(), content.clone());
        Ok(content)
    }
}

fn ref_name(session: &str, turn: u64, end: bool) -> String {
    format!(
        "refs/brigadier/s/{session}/turn-{turn}-{}",
        if end { "end" } else { "start" }
    )
}

/// A file's lines, without their ends.
fn split(content: &[u8]) -> Vec<&[u8]> {
    if content.is_empty() {
        return Vec::new();
    }
    let content = content.strip_suffix(b"\n").unwrap_or(content);
    content.split(|b| *b == b'\n').collect()
}

fn trim(line: &[u8]) -> &[u8] {
    line.trim_ascii()
}

/// For each line of `new`, the line of `old` it was kept from, by the shortest edit script
/// (Myers); `None` for a line the edit added.
pub(crate) fn line_map(old: &[&[u8]], new: &[&[u8]]) -> Vec<Option<usize>> {
    let mut map = vec![None; new.len()];
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    for (at, slot) in map.iter_mut().enumerate().take(prefix) {
        *slot = Some(at);
    }
    for back in 1..=suffix {
        map[new.len() - back] = Some(old.len() - back);
    }
    let a = &old[prefix..old.len() - suffix];
    let b = &new[prefix..new.len() - suffix];
    for (x, y) in myers(a, b) {
        map[prefix + y] = Some(prefix + x);
    }
    map
}

/// The lines `a` and `b` share by the shortest edit script, as (a, b) index pairs; none when
/// it takes more than [`MAX_EDITS`] edits.
fn myers(a: &[&[u8]], b: &[&[u8]]) -> Vec<(usize, usize)> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    if n == 0 || m == 0 {
        return Vec::new();
    }
    let max = (n + m) as usize;
    let offset = max as isize + 1;
    let mut v = vec![0isize; 2 * max + 3];
    // Each step's v for k in -d-1..=d+1, before that step.
    let mut trace: Vec<Vec<isize>> = Vec::new();
    let mut found = None;
    'search: for d in 0..=max.min(MAX_EDITS) as isize {
        let lo = (offset - d - 1) as usize;
        trace.push(v[lo..=(offset + d + 1) as usize].to_vec());
        let mut k = -d;
        while k <= d {
            let at = (offset + k) as usize;
            let mut x = if k == -d || (k != d && v[at - 1] < v[at + 1]) {
                v[at + 1]
            } else {
                v[at - 1] + 1
            };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[at] = x;
            if x >= n && y >= m {
                found = Some(d);
                break 'search;
            }
            k += 2;
        }
    }
    let Some(depth) = found else {
        return Vec::new();
    };
    let mut pairs = Vec::new();
    let (mut x, mut y) = (n, m);
    for d in (0..=depth).rev() {
        let row = &trace[d as usize];
        let get = |k: isize| row[(k + d + 1) as usize];
        let k = x - y;
        let prev_k = if k == -d || (k != d && get(k - 1) < get(k + 1)) {
            k + 1
        } else {
            k - 1
        };
        let prev_x = if d == 0 { 0 } else { get(prev_k) };
        let prev_y = if d == 0 { 0 } else { prev_x - prev_k };
        while x > prev_x && y > prev_y {
            x -= 1;
            y -= 1;
            pairs.push((x as usize, y as usize));
        }
        x = prev_x;
        y = prev_y;
    }
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::tests::test_git;
    use std::fs;

    fn lines(text: &str) -> Vec<&[u8]> {
        split(text.as_bytes())
    }

    #[test]
    fn the_line_map_keeps_what_the_edit_leaves() {
        let old = lines("a\nb\nc\nd\n");
        assert_eq!(
            line_map(&old, &lines("a\nx\nc\nd\ny\n")),
            [Some(0), None, Some(2), Some(3), None]
        );
        // A swap keeps one half, as git's diff does.
        assert_eq!(
            line_map(&old, &lines("c\nd\na\nb\n")),
            [Some(2), Some(3), None, None]
        );
        assert_eq!(line_map(&[], &lines("a\n")), [None]);
        assert_eq!(
            line_map(&lines("x\na\ny\nb\nz\n"), &lines("a\nb\n")),
            [Some(1), Some(3)]
        );
    }

    fn fixture(name: &str) -> (crate::repo::tests::Temp, Repo) {
        let (dir, git) = test_git(&format!("why-{name}"));
        let root = dir.join("repo");
        git.init(&root).unwrap();
        let repo = git.open(&root).unwrap();
        repo.cmd(&["checkout", "-b", "main"], false).unwrap();
        fs::write(root.join("app.rs"), "fn main() {\n    start();\n}\n").unwrap();
        repo.commit_changes("Start", true).unwrap();
        (dir, repo)
    }

    /// One turn of session `session`: `edit` runs between its start and end trees.
    fn turn(repo: &Repo, session: &str, number: u64, edit: impl FnOnce(&Repo)) -> WhyTurn {
        repo.snapshot_turn(session, number, false).unwrap();
        edit(repo);
        repo.snapshot_turn(session, number, true).unwrap();
        WhyTurn {
            session: session.into(),
            turn: number,
        }
    }

    fn write(repo: &Repo, path: &str, text: &str) {
        fs::write(repo.root.join(path), text).unwrap();
    }

    fn by_turn(why: &WhyLine) -> (&str, u64, usize) {
        match &why.origin {
            Origin::Turn(origin) => (&origin.turn.session, origin.turn.turn, origin.line),
            other => panic!("not a turn's: {other:?}"),
        }
    }

    #[test]
    fn a_line_is_the_turn_that_introduced_it_and_a_later_edit_is_the_later_turn() {
        let (_dir, repo) = fixture("modified");
        let one = turn(&repo, "s1", 1, |repo| {
            write(
                repo,
                "app.rs",
                "fn main() {\n    start();\n    let x = 1;\n    let y = 2;\n}\n",
            );
        });
        // Between turns, a line above shifts what follows.
        write(
            &repo,
            "app.rs",
            "// top\nfn main() {\n    start();\n    let x = 1;\n    let y = 2;\n}\n",
        );
        let two = turn(&repo, "s1", 2, |repo| {
            write(
                repo,
                "app.rs",
                "// top\nfn main() {\n    start();\n    let x = 1;\n    let y = 3;\n}\n",
            );
        });
        let turns = [two, one];
        let x = repo.why_line("app.rs", 4, &turns).unwrap();
        assert_eq!(x.text, "    let x = 1;");
        assert_eq!(by_turn(&x), ("s1", 1, 3));
        let y = repo.why_line("app.rs", 5, &turns).unwrap();
        assert_eq!(by_turn(&y), ("s1", 2, 5));
        // The line above came from no turn and isn't committed.
        let top = repo.why_line("app.rs", 1, &turns).unwrap();
        assert_eq!(top.origin, Origin::Outside(None));
        // Older than every turn: the user's own commit.
        let start = repo.why_line("app.rs", 3, &turns).unwrap();
        let Origin::Outside(Some(commit)) = start.origin else {
            panic!("{start:?}");
        };
        assert_eq!(commit.summary, "Start");
        assert_eq!(commit.author, "Test");
    }

    #[test]
    fn a_moved_line_is_still_the_turn_that_wrote_it() {
        let (_dir, repo) = fixture("moved");
        write(&repo, "lib.rs", "fn other() {}\n");
        let one = turn(&repo, "s1", 1, |repo| {
            write(
                repo,
                "app.rs",
                "fn main() {\n    start();\n    helper_call(42);\n}\n",
            );
        });
        // A second session's turn moves it into another file, and adds a line beside it.
        let two = turn(&repo, "s2", 1, |repo| {
            write(repo, "app.rs", "fn main() {\n    start();\n}\n");
            write(
                repo,
                "lib.rs",
                "fn other() {}\nfn moved() {\n    helper_call(42);\n}\n",
            );
        });
        let turns = [two.clone(), one];
        let why = repo.why_line("lib.rs", 3, &turns).unwrap();
        let Origin::Turn(origin) = &why.origin else {
            panic!("{why:?}");
        };
        assert_eq!((origin.turn.session.as_str(), origin.turn.turn), ("s1", 1));
        assert_eq!((origin.path.as_str(), origin.line), ("app.rs", 3));
        assert_eq!(origin.moved_by, [two]);
        // What the moving turn wrote itself stays its own.
        let new = repo.why_line("lib.rs", 2, &turns).unwrap();
        assert_eq!(by_turn(&new), ("s2", 1, 2));
    }

    #[test]
    fn a_users_commit_after_the_turns_is_git_history() {
        let (_dir, repo) = fixture("user-commit");
        let one = turn(&repo, "s1", 1, |repo| {
            write(
                repo,
                "app.rs",
                "fn main() {\n    start();\n    by_turn();\n}\n",
            );
        });
        write(
            &repo,
            "app.rs",
            "fn main() {\n    start();\n    by_turn();\n    by_user();\n}\n",
        );
        repo.commit_changes("The user's own", true).unwrap();
        let turns = [one];
        let user = repo.why_line("app.rs", 4, &turns).unwrap();
        let Origin::Outside(Some(commit)) = &user.origin else {
            panic!("{user:?}");
        };
        assert_eq!(commit.summary, "The user's own");
        assert_eq!(
            by_turn(&repo.why_line("app.rs", 3, &turns).unwrap()),
            ("s1", 1, 3)
        );
        // A file no turn and no commit has.
        write(&repo, "notes.txt", "loose\n");
        assert_eq!(
            repo.why_line("notes.txt", 1, &turns).unwrap().origin,
            Origin::Outside(None)
        );
        assert!(repo.why_line("app.rs", 9, &turns).is_err());
    }

    #[test]
    fn a_deleted_sessions_turns_are_passed_over() {
        let (_dir, repo) = fixture("deleted");
        let one = turn(&repo, "s1", 1, |repo| {
            write(
                repo,
                "app.rs",
                "fn main() {\n    start();\n    first();\n}\n",
            );
        });
        let two = turn(&repo, "s2", 1, |repo| {
            write(
                repo,
                "app.rs",
                "fn main() {\n    start();\n    first();\n    second();\n}\n",
            );
        });
        let turns = [two, one];
        assert_eq!(
            by_turn(&repo.why_line("app.rs", 4, &turns).unwrap()),
            ("s2", 1, 4)
        );
        // Delete takes a session's refs with it: what it wrote is no turn's any more.
        repo.delete_session_snapshots("s2").unwrap();
        assert_eq!(
            repo.why_line("app.rs", 4, &turns).unwrap().origin,
            Origin::Outside(None)
        );
        assert_eq!(
            by_turn(&repo.why_line("app.rs", 3, &turns).unwrap()),
            ("s1", 1, 3)
        );
    }

    #[test]
    fn a_line_a_turn_committed_from_a_merged_branch_names_the_branch() {
        let (_dir, repo) = fixture("landed");
        repo.cmd(&["branch", "worker-1"], false).unwrap();
        let one = turn(&repo, "s1", 1, |repo| {
            repo.cmd(&["checkout", "-q", "worker-1"], false).unwrap();
            write(
                repo,
                "app.rs",
                "fn main() {\n    start();\n    from_worker();\n}\n",
            );
            repo.commit_changes("Worker's change", true).unwrap();
            repo.cmd(&["checkout", "-q", "main"], false).unwrap();
            write(repo, "own.txt", "parent\n");
            repo.commit_changes("Parent's own", true).unwrap();
            repo.cmd(&["merge", "-q", "--no-edit", "worker-1"], false)
                .unwrap();
        });
        let turns = [one];
        let why = repo.why_line("app.rs", 3, &turns).unwrap();
        let Origin::Turn(origin) = &why.origin else {
            panic!("{why:?}");
        };
        let commit = origin.commit.as_ref().expect("committed in the turn");
        assert_eq!(commit.commit.summary, "Worker's change");
        assert_eq!(commit.branch.as_deref(), Some("worker-1"));
        assert!(commit.merge.is_some());
        let own = repo.why_line("own.txt", 1, &turns).unwrap();
        let Origin::Turn(origin) = &own.origin else {
            panic!("{own:?}");
        };
        let commit = origin.commit.as_ref().expect("committed in the turn");
        assert_eq!(commit.commit.summary, "Parent's own");
        assert_eq!(commit.branch, None);
        assert_eq!(
            repo.snapshot_turns().unwrap(),
            [WhyTurn {
                session: "s1".into(),
                turn: 1
            }]
        );
    }
}
