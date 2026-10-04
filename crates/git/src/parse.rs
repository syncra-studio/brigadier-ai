use crate::{
    Change, ChangeKind, CollisionKind, DiffStat, Error, FileStat, Oid, Result, WorktreeInfo,
};
use std::{collections::BTreeSet, path::PathBuf};

pub(crate) fn text(bytes: &[u8]) -> Result<&str> {
    std::str::from_utf8(bytes)
        .map_err(|_| Error::Parse("non-UTF-8 git output/path is not supported".into()))
}
pub(crate) fn line(bytes: &[u8]) -> Result<String> {
    Ok(text(bytes)?
        .strip_suffix('\n')
        .unwrap_or(text(bytes)?)
        .to_owned())
}
pub(crate) fn path_line(bytes: &[u8]) -> Result<PathBuf> {
    let bytes = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Ok(PathBuf::from(std::ffi::OsStr::from_bytes(bytes)))
    }
    #[cfg(not(unix))]
    {
        Ok(PathBuf::from(text(bytes)?))
    }
}
pub(crate) fn oid(bytes: &[u8]) -> Result<Oid> {
    let id = Oid(line(bytes)?);
    crate::command::valid_oid(&id)
        .map_err(|_| Error::Parse(format!("invalid object id: {}", id.0)))?;
    Ok(id)
}
pub(crate) fn fields(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    bytes.split(|&c| c == 0)
}

#[derive(Default)]
pub(crate) struct Status {
    pub branch: Option<String>,
    pub head: Option<Oid>,
    pub entries: Vec<(String, CollisionKind)>,
    pub unmerged: bool,
    pub dirty_submodule: bool,
}
impl Status {
    pub fn dirty(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter(|(_, k)| *k != CollisionKind::Ignored)
            .map(|(p, _)| p.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }
    pub fn untracked(&self) -> BTreeSet<String> {
        self.entries
            .iter()
            .filter(|(_, k)| *k == CollisionKind::Untracked)
            .map(|(p, _)| p.trim_end_matches('/').to_owned())
            .collect()
    }
}

pub(crate) fn status(bytes: &[u8]) -> Result<Status> {
    let mut status = Status::default();
    let mut records = fields(bytes);
    while let Some(record) = records.next() {
        if record.is_empty() {
            continue;
        }
        let s = text(record)?;
        if let Some(branch) = s.strip_prefix("# branch.head ") {
            if branch != "(detached)" {
                status.branch = Some(branch.to_owned());
            }
        } else if let Some(head) = s.strip_prefix("# branch.oid ") {
            if head != "(initial)" {
                status.head = Some(oid(head.as_bytes())?);
            }
        } else if s.starts_with("# ") {
            continue;
        } else if let Some(path) = s.strip_prefix("? ") {
            status
                .entries
                .push((path.to_owned(), CollisionKind::Untracked));
        } else if let Some(path) = s.strip_prefix("! ") {
            status
                .entries
                .push((path.to_owned(), CollisionKind::Ignored));
        } else {
            let count = match record[0] {
                b'1' => 9,
                b'2' => 10,
                b'u' => {
                    status.unmerged = true;
                    11
                }
                _ => return Err(Error::Parse(s.into())),
            };
            let parts: Vec<_> = s.splitn(count, ' ').collect();
            if parts.len() != count {
                return Err(Error::Parse(s.into()));
            }
            if parts[2].starts_with('S')
                && parts[2]
                    .as_bytes()
                    .get(2..)
                    .is_some_and(|flags| flags.iter().any(|&b| b != b'.'))
            {
                status.dirty_submodule = true;
            }
            status
                .entries
                .push((parts[count - 1].to_owned(), CollisionKind::Modified));
            if record[0] == b'2' {
                let from = records
                    .next()
                    .ok_or_else(|| Error::Parse("missing rename source".into()))?;
                status
                    .entries
                    .push((text(from)?.to_owned(), CollisionKind::Modified));
            }
        }
    }
    Ok(status)
}

pub(crate) fn worktrees(bytes: &[u8]) -> Result<Vec<WorktreeInfo>> {
    let mut result = Vec::new();
    let mut current: Option<WorktreeInfo> = None;
    for record in fields(bytes) {
        if record.is_empty() {
            if let Some(item) = current.take() {
                result.push(item);
            }
            continue;
        }
        if let Some(path) = record.strip_prefix(b"worktree ") {
            if current.is_some() {
                return Err(Error::Parse("worktree record missing separator".into()));
            }
            // Unlike rev-parse, this field is NUL terminated, so preserve trailing newlines.
            #[cfg(unix)]
            let path = {
                use std::os::unix::ffi::OsStrExt;
                PathBuf::from(std::ffi::OsStr::from_bytes(path))
            };
            #[cfg(not(unix))]
            let path = PathBuf::from(text(path)?);
            current = Some(WorktreeInfo {
                path,
                head: None,
                branch: None,
                locked: false,
                prunable: false,
            });
        } else if let Some(item) = current.as_mut() {
            let s = text(record)?;
            if let Some(head) = s.strip_prefix("HEAD ") {
                if !head.bytes().all(|c| c == b'0') {
                    item.head = Some(oid(head.as_bytes())?);
                }
            } else if let Some(branch) = s.strip_prefix("branch refs/heads/") {
                item.branch = Some(branch.to_owned());
            } else if s == "locked" || s.starts_with("locked ") {
                item.locked = true;
            } else if s == "prunable" || s.starts_with("prunable ") {
                item.prunable = true;
            }
        } else {
            return Err(Error::Parse("worktree field before path".into()));
        }
    }
    if let Some(item) = current {
        result.push(item);
    }
    Ok(result)
}

pub(crate) fn changes(bytes: &[u8], untracked: &BTreeSet<String>) -> Result<Vec<Change>> {
    let mut result = Vec::new();
    let mut records = fields(bytes);
    while let Some(code) = records.next() {
        if code.is_empty() {
            continue;
        }
        let path = text(
            records
                .next()
                .ok_or_else(|| Error::Parse("missing diff path".into()))?,
        )?
        .to_owned();
        let (kind, path) = match code[0] {
            b'A' => (ChangeKind::Added, path),
            b'M' => (ChangeKind::Modified, path),
            b'D' => (ChangeKind::Deleted, path),
            b'T' => (ChangeKind::TypeChanged, path),
            b'R' => {
                let to = text(
                    records
                        .next()
                        .ok_or_else(|| Error::Parse("missing rename target".into()))?,
                )?
                .to_owned();
                (ChangeKind::Renamed { from: path }, to)
            }
            _ => {
                return Err(Error::Parse(format!(
                    "unexpected diff status {}",
                    text(code)?
                )));
            }
        };
        result.push(Change {
            untracked: untracked.contains(&path),
            path,
            kind,
        });
    }
    result.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(result)
}

pub(crate) fn stat(bytes: &[u8]) -> Result<DiffStat> {
    read_stat(bytes, false).map(|(stat, _)| stat)
}

pub(crate) fn stat_with_paths(bytes: &[u8]) -> Result<(DiffStat, Vec<String>)> {
    read_stat(bytes, true)
}

fn read_stat(bytes: &[u8], collect_paths: bool) -> Result<(DiffStat, Vec<String>)> {
    let mut result = DiffStat::default();
    let mut paths = std::collections::BTreeSet::new();
    let mut records = fields(bytes);
    while let Some(record) = records.next() {
        if record.is_empty() {
            continue;
        }
        let parts: Vec<_> = text(record)?.splitn(3, '\t').collect();
        if parts.len() != 3 {
            return Err(Error::Parse("invalid numstat record".into()));
        }
        let path = if parts[2].is_empty() {
            let source = records
                .next()
                .ok_or_else(|| Error::Parse("missing numstat rename source".into()))?;
            if collect_paths {
                if source.is_empty() {
                    return Err(Error::Parse("empty numstat rename source".into()));
                }
                paths.insert(text(source)?.to_owned());
            }
            text(
                records
                    .next()
                    .ok_or_else(|| Error::Parse("missing numstat rename target".into()))?,
            )?
        } else {
            parts[2]
        };
        if collect_paths {
            if path.is_empty() {
                return Err(Error::Parse("empty numstat path".into()));
            }
            paths.insert(path.to_owned());
        }
        let binary = parts[0] == "-" && parts[1] == "-";
        let number = |s: &str| {
            if binary {
                Ok(0)
            } else {
                s.parse::<u32>()
                    .map_err(|_| Error::Parse("invalid line count".into()))
            }
        };
        let insertions = number(parts[0])?;
        let deletions = number(parts[1])?;
        result.insertions = result
            .insertions
            .checked_add(insertions)
            .ok_or_else(|| Error::Parse("line count overflow".into()))?;
        result.deletions = result
            .deletions
            .checked_add(deletions)
            .ok_or_else(|| Error::Parse("line count overflow".into()))?;
        result.files.push(FileStat {
            path: path.into(),
            insertions,
            deletions,
            binary,
        });
    }
    result.files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok((result, paths.into_iter().collect()))
}

pub(crate) fn overlaps(a: &str, b: &str) -> bool {
    let a = a.trim_end_matches('/');
    let b = b.trim_end_matches('/');
    a == b
        || a.strip_prefix(b).is_some_and(|s| s.starts_with('/'))
        || b.strip_prefix(a).is_some_and(|s| s.starts_with('/'))
}

#[cfg(test)]
mod scope_tests {
    use super::*;

    #[test]
    fn scope_preserves_rename_endpoints_and_existing_stats() {
        for bytes in [
            b"0\t0\t\0crates/git/a.rs\0src/a.rs\0".as_slice(),
            b"0\t0\t\0src/a.rs\0Cargo.toml\0".as_slice(),
        ] {
            let (details, paths) = stat_with_paths(bytes).unwrap();
            assert_eq!(details, stat(bytes).unwrap());
            assert_eq!(paths.len(), 2);
            assert!(paths.iter().any(|p| p == "src/a.rs"));
        }
    }

    #[test]
    fn scope_keeps_binary_and_rejects_unknown_counts() {
        let (stat, _) = stat_with_paths(b"-\t-\timage.png\0").unwrap();
        assert!(stat.files[0].binary);
        assert!(stat_with_paths(b"?\t0\tsrc/a.rs\0").is_err());
        assert!(stat_with_paths(b"0\t0\t\0old\0").is_err());
    }
}
