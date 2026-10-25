//! Taking lines out of a CLI's JSON-lines file (a prompt history, a thread index) that other
//! processes keep appending to while we rewrite it.
//!
//! The kept lines are written to a new file next to the original, which then replaces it in
//! one rename, so a reader sees the old file or the new one, never half of either. A CLI that
//! appends meanwhile writes to the old file: what it adds before the rename is copied too, and
//! what it adds after, read back through the old file's handle, is appended to the new one.

use std::fs::{File, OpenOptions};
use std::io::{self, Read as _, Seek as _, SeekFrom, Write as _};
use std::path::Path;
use std::sync::Mutex;

use serde_json::Value;

/// One rewrite at a time in this process: two at once would each restore what the other took
/// out.
static REWRITING: Mutex<()> = Mutex::new(());

/// Removes every line of `file` that is a JSON value `drop` accepts. Other lines (those that
/// aren't JSON too) stay byte for byte, and so does whether the file ends in a newline. Nothing
/// happens when the file is missing or no line matches. A link is followed: the file it points
/// to is rewritten, the link stays.
pub(crate) fn remove_lines(file: &Path, drop: impl Fn(&Value) -> bool) -> io::Result<()> {
    remove_lines_with(file, &drop, &mut |_| {})
}

/// Where a rewrite stands, for tests that append to the file on the way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// The first read is copied; what was appended since is read next.
    Copied,
    /// The new file is complete and about to replace the old one.
    Renaming,
}

fn remove_lines_with(
    file: &Path,
    drop: &dyn Fn(&Value) -> bool,
    at: &mut dyn FnMut(Stage),
) -> io::Result<()> {
    let _rewriting = REWRITING.lock().unwrap_or_else(|e| e.into_inner());
    let path = match std::fs::canonicalize(file) {
        Ok(path) => path,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };
    let mut old = File::open(&path)?;
    let meta = old.metadata()?;
    if !meta.is_file() {
        return Ok(());
    }
    let mut data = Vec::new();
    old.read_to_end(&mut data)?;
    if !data
        .split(|&byte| byte == b'\n')
        .any(|line| dropped(line, drop))
    {
        return Ok(());
    }
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
        return Ok(());
    };
    let temp = dir.join(format!(
        ".{}.{}.tmp",
        name.to_string_lossy(),
        uuid::Uuid::new_v4().simple()
    ));
    // Appending: once it replaces the original, a CLI may append to it too.
    let mut new = OpenOptions::new()
        .append(true)
        .create_new(true)
        .open(&temp)?;
    let written = (|| {
        new.set_permissions(meta.permissions())?;
        let mut filter = Filter::default();
        new.write_all(&filter.feed(&data, drop))?;
        let mut copied = data.len() as u64;
        at(Stage::Copied);
        // What was appended while we copied.
        loop {
            let more = read_from(&mut old, copied)?;
            if more.is_empty() {
                break;
            }
            copied += more.len() as u64;
            new.write_all(&filter.feed(&more, drop))?;
        }
        new.sync_all()?;
        at(Stage::Renaming);
        std::fs::rename(&temp, &path)?;
        // What was appended to the old file after the last read, through to the rename.
        loop {
            let more = read_from(&mut old, copied)?;
            if more.is_empty() {
                break;
            }
            copied += more.len() as u64;
            new.write_all(&filter.feed(&more, drop))?;
        }
        new.write_all(&filter.finish(drop))?;
        new.sync_all()
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
}

fn read_from(file: &mut File, at: u64) -> io::Result<Vec<u8>> {
    file.seek(SeekFrom::Start(at))?;
    let mut more = Vec::new();
    file.read_to_end(&mut more)?;
    Ok(more)
}

/// Whether `line` (without its newline) is JSON that `drop` accepts.
fn dropped(line: &[u8], drop: &dyn Fn(&Value) -> bool) -> bool {
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    !line.iter().all(u8::is_ascii_whitespace)
        && serde_json::from_slice::<Value>(line).is_ok_and(|value| drop(&value))
}

/// Filters a file read in pieces: a line split between two reads is held until it is whole.
#[derive(Default)]
struct Filter {
    partial: Vec<u8>,
}

impl Filter {
    /// The kept whole lines of `bytes` (with what came before them), newlines included.
    fn feed(&mut self, bytes: &[u8], drop: &dyn Fn(&Value) -> bool) -> Vec<u8> {
        self.partial.extend_from_slice(bytes);
        let Some(end) = self.partial.iter().rposition(|&byte| byte == b'\n') else {
            return Vec::new();
        };
        let rest = self.partial.split_off(end + 1);
        let whole = std::mem::replace(&mut self.partial, rest);
        let mut kept = Vec::with_capacity(whole.len());
        for line in whole.split_inclusive(|&byte| byte == b'\n') {
            if !dropped(&line[..line.len() - 1], drop) {
                kept.extend_from_slice(line);
            }
        }
        kept
    }

    /// The last line, when the file doesn't end in a newline, unless it is dropped.
    fn finish(&mut self, drop: &dyn Fn(&Value) -> bool) -> Vec<u8> {
        let last = std::mem::take(&mut self.partial);
        if dropped(&last, drop) {
            Vec::new()
        } else {
            last
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("brigadier-lines-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn of(id: &str) -> impl Fn(&Value) -> bool + '_ {
        move |value: &Value| value.get("sessionId").and_then(Value::as_str) == Some(id)
    }

    fn append(path: &Path, text: &str) {
        OpenOptions::new()
            .append(true)
            .open(path)
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
    }

    #[test]
    fn only_the_matching_lines_go_and_the_rest_stay_byte_for_byte() {
        let dir = scratch();
        let file = dir.join("history.jsonl");
        let text = concat!(
            "{\"display\":\"a\",\"sessionId\":\"x\"}\n",
            "{\"display\":  \"b\" , \"sessionId\":\"y\"}\n",
            "not json at all\n",
            "{\"display\":\"c\",\"pastedContents\":{},\"sessionId\":\"x\"}\r\n",
            "\n",
            "{\"display\":\"d\",\"sessionId\":\"y\"}",
        );
        std::fs::write(&file, text).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        remove_lines(&file, of("x")).unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            concat!(
                "{\"display\":  \"b\" , \"sessionId\":\"y\"}\n",
                "not json at all\n",
                "\n",
                "{\"display\":\"d\",\"sessionId\":\"y\"}",
            )
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&file).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        // Nothing matches: the file isn't rewritten.
        let before = std::fs::metadata(&file).unwrap().modified().unwrap();
        remove_lines(&file, of("x")).unwrap();
        assert_eq!(
            std::fs::metadata(&file).unwrap().modified().unwrap(),
            before
        );
        // A missing file is fine.
        remove_lines(&dir.join("missing.jsonl"), of("x")).unwrap();
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "no temp left");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_last_line_dropped_keeps_the_newline_before_it() {
        let dir = scratch();
        let file = dir.join("history.jsonl");
        std::fs::write(&file, "{\"sessionId\":\"y\"}\n{\"sessionId\":\"x\"}").unwrap();
        remove_lines(&file, of("x")).unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "{\"sessionId\":\"y\"}\n"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn lines_appended_while_it_rewrites_survive_and_are_filtered_too() {
        let dir = scratch();
        let file = dir.join("history.jsonl");
        std::fs::write(
            &file,
            "{\"sessionId\":\"x\"}\n{\"sessionId\":\"y\",\"n\":1}\n{\"sess",
        )
        .unwrap();
        let path = file.clone();
        remove_lines_with(&file, &of("x"), &mut |stage| match stage {
            // Finishes the line that was half written, and adds more.
            Stage::Copied => append(
                &path,
                "ionId\":\"y\",\"n\":2}\n{\"sessionId\":\"x\"}\n{\"sessionId\":\"y\",\"n\":3}\n",
            ),
            // Lands in the old file after the last read: read back after the rename.
            Stage::Renaming => append(
                &path,
                "{\"sessionId\":\"x\"}\n{\"sessionId\":\"y\",\"n\":4}\n",
            ),
        })
        .unwrap();
        // And a CLI appending to the new file afterwards.
        append(&file, "{\"sessionId\":\"y\",\"n\":5}\n");
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            concat!(
                "{\"sessionId\":\"y\",\"n\":1}\n",
                "{\"sessionId\":\"y\",\"n\":2}\n",
                "{\"sessionId\":\"y\",\"n\":3}\n",
                "{\"sessionId\":\"y\",\"n\":4}\n",
                "{\"sessionId\":\"y\",\"n\":5}\n",
            )
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_file_is_rewritten_where_it_is() {
        let dir = scratch();
        let file = dir.join("real.jsonl");
        let link = dir.join("history.jsonl");
        std::fs::write(&file, "{\"sessionId\":\"x\"}\n{\"sessionId\":\"y\"}\n").unwrap();
        std::os::unix::fs::symlink(&file, &link).unwrap();
        remove_lines(&link, of("x")).unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "{\"sessionId\":\"y\"}\n"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
