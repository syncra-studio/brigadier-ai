//! Site storage: where single-page apps keep the tokens that aren't cookies. A sign-in whose cookie
//! renews from a token in `Local Storage` or `IndexedDB` ends minutes after an import that brought
//! cookies alone. Chromium keeps these folders unencrypted, and clears what it can't read.

use std::path::Path;

use crate::Error;
use crate::sqlite::io_error;

/// The folders of a Chromium profile that are copied.
pub const SITE_STORAGE_FOLDERS: [&str; 2] = ["Local Storage", "IndexedDB"];
/// Lock files belong to whoever has the store open, and aren't data.
const NOT_COPIED: [&str; 1] = ["LOCK"];

/// Copies a Chromium profile's site storage from `from` into `target`, which no engine has open.
/// Each folder arrives whole or not at all, in place of what was there. Returns the files copied.
pub fn copy_site_storage(from: &Path, target: &Path) -> Result<usize, Error> {
    std::fs::create_dir_all(target).map_err(|err| io_error(target, err))?;
    let mut copied = 0;
    for folder in SITE_STORAGE_FOLDERS {
        let source = from.join(folder);
        if !source.is_dir() {
            continue;
        }
        let place = target.join(folder);
        let partial = target.join(format!("{folder}.importing"));
        remove_dir_if_there(&partial)?;
        copied += copy_tree(&source, &partial)?;
        remove_dir_if_there(&place)?;
        std::fs::rename(&partial, &place).map_err(|err| io_error(&place, err))?;
    }
    Ok(copied)
}

fn remove_dir_if_there(path: &Path) -> Result<(), Error> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(io_error(path, err)),
    }
}

fn copy_tree(from: &Path, to: &Path) -> Result<usize, Error> {
    std::fs::create_dir_all(to).map_err(|err| io_error(to, err))?;
    let mut copied = 0;
    for entry in std::fs::read_dir(from).map_err(|err| io_error(from, err))? {
        let entry = entry.map_err(|err| io_error(from, err))?;
        let kind = entry.file_type().map_err(|err| io_error(&entry.path(), err))?;
        let name = entry.file_name();
        if kind.is_dir() {
            copied += copy_tree(&entry.path(), &to.join(&name))?;
        } else if kind.is_file() && !NOT_COPIED.iter().any(|skip| name == *skip) {
            std::fs::copy(entry.path(), to.join(&name)).map_err(|err| io_error(&entry.path(), err))?;
            copied += 1;
        }
    }
    Ok(copied)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn copies_each_folder_whole_in_place_of_the_old_and_leaves_lock_files() {
        let dir = tempfile::tempdir().unwrap();
        let (from, target) = (dir.path().join("from"), dir.path().join("target"));
        write(&from.join("Local Storage/leveldb/000003.log"), "new");
        write(&from.join("Local Storage/leveldb/LOCK"), "");
        write(&from.join("Cookies"), "not site storage");
        write(&target.join("Local Storage/leveldb/old.log"), "old");
        write(&target.join("IndexedDB/kept"), "the source has none");

        assert_eq!(copy_site_storage(&from, &target).unwrap(), 1);

        let leveldb = target.join("Local Storage/leveldb");
        let mut names: Vec<_> = std::fs::read_dir(&leveldb)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        names.sort();
        assert_eq!(names, ["000003.log"]);
        assert!(target.join("IndexedDB/kept").exists());
        assert!(!target.join("Cookies").exists());
        assert!(!target.join("Local Storage.importing").exists());
    }
}
