//! What a deleted session leaves in Brigadier's own log files: the lines naming it go, so
//! that nothing of it is left once its Delete finishes (D8).

use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Mutex;

/// The shortest needle used: anything shorter could match lines that don't name the session.
const SHORTEST: usize = 8;

/// One scrub at a time: two deletes' scrubs of one file would each write back what the other
/// took out.
static SCRUBBING: Mutex<()> = Mutex::new(());

/// Removes, from every `.log` file directly in `dir`, the lines that contain one of `needles`.
/// A file is rewritten in place, so the daemon's writer, which appends to it, goes on writing
/// to the same file; a line it appends while the file is read is kept. Returns how many lines
/// went.
pub(crate) fn scrub_logs(dir: &Path, needles: &[String]) -> io::Result<usize> {
    let _scrubbing = SCRUBBING.lock().unwrap_or_else(|e| e.into_inner());
    let needles: Vec<&[u8]> = needles
        .iter()
        .filter(|needle| needle.len() >= SHORTEST)
        .map(|needle| needle.as_bytes())
        .collect();
    if needles.is_empty() {
        return Ok(0);
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(err) => return Err(err),
    };
    let mut dropped = 0;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if !entry.file_type()?.is_file() || path.extension().is_none_or(|ext| ext != "log") {
            continue;
        }
        dropped += scrub_file(&path, &needles)?;
    }
    Ok(dropped)
}

fn names(line: &[u8], needles: &[&[u8]]) -> bool {
    needles
        .iter()
        .any(|needle| memchr::memmem::find(line, needle).is_some())
}

fn scrub_file(path: &Path, needles: &[&[u8]]) -> io::Result<usize> {
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .append(true)
        .open(path)?;
    let mut text = Vec::new();
    file.read_to_end(&mut text)?;
    if !needles
        .iter()
        .any(|needle| memchr::memmem::find(&text, needle).is_some())
    {
        return Ok(0);
    }
    // What was appended while it was read is read too, right before the rewrite.
    loop {
        let len = file.metadata()?.len();
        if len <= text.len() as u64 {
            break;
        }
        file.seek(SeekFrom::Start(text.len() as u64))?;
        file.read_to_end(&mut text)?;
    }
    let mut kept = Vec::with_capacity(text.len());
    let mut dropped = 0;
    for line in text.split_inclusive(|b| *b == b'\n') {
        if names(line, needles) {
            dropped += 1;
        } else {
            kept.extend_from_slice(line);
        }
    }
    file.set_len(0)?;
    file.write_all(&kept)?;
    file.sync_all()?;
    Ok(dropped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_lines_naming_the_session_go() {
        let dir = std::env::temp_dir().join(format!(
            "brigadier-20261025-1221-cleanup-on-delete-w01-logs-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("brigadierd.2026-10-25.log");
        let other = dir.join("notes.txt");
        let id = "01a171b6-fecf-7061-9ebe-0314ecc1e15f";
        std::fs::write(
            &log,
            format!("{{\"a\":1}}\n{{\"conversation\":\"{id}\"}}\n{{\"b\":2}}\n{{\"path\":\"/s/ecc1e15f/work\"}}"),
        )
        .unwrap();
        std::fs::write(&other, id).unwrap();
        let needles = vec![id.to_owned(), "/s/ecc1e15f/".to_owned(), "short".to_owned()];
        assert_eq!(scrub_logs(&dir, &needles).unwrap(), 2);
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            "{\"a\":1}\n{\"b\":2}\n"
        );
        // Not a log file: untouched. Twice: nothing more.
        assert_eq!(std::fs::read_to_string(&other).unwrap(), id);
        assert_eq!(scrub_logs(&dir, &needles).unwrap(), 0);
        assert_eq!(scrub_logs(&dir.join("missing"), &needles).unwrap(), 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn two_scrubs_at_once_both_take_their_lines() {
        let dir = std::env::temp_dir().join(format!(
            "brigadier-20261025-1221-cleanup-on-delete-w04-logs-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("brigadierd.log");
        let (a, b) = ("aaaaaaaa-session", "bbbbbbbb-session");
        let mut text = String::new();
        for n in 0..20_000 {
            text.push_str(&format!(
                "{{\"n\":{n},\"id\":\"{}\"}}\n",
                [a, b, "kept"][n % 3]
            ));
        }
        std::fs::write(&log, text).unwrap();
        std::thread::scope(|scope| {
            for id in [a, b] {
                let dir = &dir;
                scope.spawn(move || scrub_logs(dir, &[id.to_owned()]).unwrap());
            }
        });
        let left = std::fs::read_to_string(&log).unwrap();
        assert!(!left.contains(a) && !left.contains(b));
        assert_eq!(left.lines().count(), 6_666);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
