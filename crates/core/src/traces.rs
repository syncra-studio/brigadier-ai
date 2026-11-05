//! What a deleted session leaves in Brigadier's own log files: the lines naming it go, so
//! that nothing of it is left once its Delete finishes (D8).

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// The shortest needle used: anything shorter could match lines that don't name the session.
const SHORTEST: usize = 8;

/// One scrub at a time: two deletes' scrubs of one file would each write back what the other
/// took out.
static SCRUBBING: Mutex<()> = Mutex::new(());

static SWAPS: AtomicU64 = AtomicU64::new(0);

/// How many log files scrubs have replaced so far. A writer holding a log file open reopens it
/// when this changes, or it would go on writing to the replaced file, which nothing can read.
pub fn swaps() -> u64 {
    SWAPS.load(Ordering::Acquire)
}

/// Removes, from every `.log` file directly in `dir`, the lines that contain one of `needles`.
/// The kept lines go to a new file that replaces the old one in one rename, so a crash leaves
/// one or the other whole; a line the daemon's writer appends meanwhile is kept, and [`swaps`]
/// moves it to the new file. Returns how many lines went.
pub fn scrub_logs(dir: &Path, needles: &[String]) -> io::Result<usize> {
    let _scrubbing = SCRUBBING.lock().unwrap_or_else(|e| e.into_inner());
    let needles: Vec<&[u8]> = needles
        .iter()
        .filter(|needle| needle.len() >= SHORTEST)
        .map(|needle| needle.as_bytes())
        .collect();
    if needles.is_empty() {
        return Ok(0);
    }
    // Listed first: creating and renaming files while reading a folder can skip some of them.
    let entries: io::Result<Vec<_>> = match std::fs::read_dir(dir) {
        Ok(entries) => entries.collect(),
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(err) => return Err(err),
    };
    let mut dropped = 0;
    for entry in entries? {
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
    scrub_file_with(path, needles, &mut || {})
}

/// `at` runs right before the new file replaces the old one, for a test to append on the way.
fn scrub_file_with(path: &Path, needles: &[&[u8]], at: &mut dyn FnMut()) -> io::Result<usize> {
    let mut old = File::open(path)?;
    let meta = old.metadata()?;
    let mut pending = Vec::new();
    old.read_to_end(&mut pending)?;
    if !needles
        .iter()
        .any(|needle| memchr::memmem::find(&pending, needle).is_some())
    {
        return Ok(0);
    }
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let temp = path.with_file_name(format!(".{name}.{}.tmp", uuid::Uuid::new_v4().simple()));
    // Appending: once it replaces the old file, the daemon's writer appends to it too.
    let mut new = OpenOptions::new()
        .append(true)
        .create_new(true)
        .open(&temp)?;
    let written = (|| {
        new.set_permissions(meta.permissions())?;
        let mut dropped = keep_whole_lines(&mut pending, needles, &mut new)?;
        // What was appended while it was read.
        old.read_to_end(&mut pending)?;
        dropped += keep_whole_lines(&mut pending, needles, &mut new)?;
        new.sync_all()?;
        at();
        std::fs::rename(&temp, path)?;
        SWAPS.fetch_add(1, Ordering::AcqRel);
        // What reached the old file up to the swap. Accepted gap: a line its writer sends there
        // after this read, before it reopens the file, is lost; no lock is shared with it.
        old.read_to_end(&mut pending)?;
        dropped += keep(&pending, needles, &mut new)?;
        new.sync_all()?;
        Ok(dropped)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
}

/// Writes the whole lines of `pending` that stay; what follows its last newline stays pending,
/// so that a line read in two pieces is judged whole.
fn keep_whole_lines(pending: &mut Vec<u8>, needles: &[&[u8]], new: &mut File) -> io::Result<usize> {
    let end = memchr::memrchr(b'\n', pending).map_or(0, |at| at + 1);
    let dropped = keep(&pending[..end], needles, new)?;
    pending.drain(..end);
    Ok(dropped)
}

/// Writes the lines of `text` that name no needle, and returns how many others there were.
fn keep(text: &[u8], needles: &[&[u8]], new: &mut File) -> io::Result<usize> {
    let mut kept = Vec::with_capacity(text.len());
    let mut dropped = 0;
    for line in text.split_inclusive(|b| *b == b'\n') {
        if names(line, needles) {
            dropped += 1;
        } else {
            kept.extend_from_slice(line);
        }
    }
    new.write_all(&kept)?;
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
    fn a_scrub_replaces_the_file_and_keeps_a_line_appended_at_the_swap() {
        let dir = std::env::temp_dir().join(format!(
            "brigadier-scrub-swap-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("brigadierd.log");
        let id = "aaaaaaaa-session";
        let original = format!("{{\"a\":1}}\n{{\"id\":\"{id}\"}}\n");
        std::fs::write(&log, &original).unwrap();
        let mut writer = OpenOptions::new().append(true).open(&log).unwrap();
        // What the old file holds is never cut: a crash at any point leaves it or the new one.
        let mut before = std::fs::File::open(&log).unwrap();
        let appended = format!("{{\"b\":2}}\n{{\"id\":\"{id}\"}}\n");
        let dropped = scrub_file_with(&log, &[id.as_bytes()], &mut || {
            writer.write_all(appended.as_bytes()).unwrap();
        });
        let mut untouched = String::new();
        before.read_to_string(&mut untouched).unwrap();
        assert_eq!(
            untouched,
            format!("{original}{appended}"),
            "the old file is not cut"
        );
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            "{\"a\":1}\n{\"b\":2}\n",
            "the line appended at the swap is kept"
        );
        assert_eq!(dropped.unwrap(), 2);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "no temp left");
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
