//! The user's own Path on Windows (`HKCU\Environment`, value `Path`), which every new shell and
//! program reads: Connect adds Claude Code's folder there once it has installed it. Claude Code's
//! `install.ps1` (`claude.exe install`) puts `claude.exe` in `%USERPROFILE%\.local\bin` and
//! leaves Path alone (it tells the user to add the folder by hand), so `claude` was "not
//! recognized" in every terminal outside Brigadier. Codex's installer adds its own folder.
//!
//! The folder joins the end of the value when no entry names it already: entries compare in any
//! case, with or without a trailing `\`, and with `%VARIABLE%`s expanded. The value keeps its
//! type (REG_EXPAND_SZ stays REG_EXPAND_SZ: Codex's installer rewrites it as REG_SZ, which
//! leaves `%VARIABLE%` entries unexpanded) and its text, byte for byte, before the `;<folder>`
//! added after it. A missing value is created as REG_EXPAND_SZ. Running programs are then told
//! the environment changed (`WM_SETTINGCHANGE`), so shells started from Explorer see it. The
//! machine's Path (HKLM) is never written.

use std::ffi::OsString;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

/// The user's environment, under `HKEY_CURRENT_USER`.
const ENVIRONMENT: &str = "Environment";

/// What adding a folder to Path did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Added {
    /// An entry named it already; nothing was written.
    Already,
    /// It was added at the end of the value.
    Appended,
    /// There was no Path value; it was created holding just the folder.
    Created,
}

/// Adds `folder` to the user's Path unless it is on it, `var` expanding `%VARIABLE%`s in its
/// entries, and tells running programs when it changed.
pub(crate) fn add(folder: &Path, var: impl Fn(&str) -> Option<OsString>) -> io::Result<Added> {
    let added = add_to(ENVIRONMENT, folder, var)?;
    if added != Added::Already {
        registry::announce_environment();
    }
    Ok(added)
}

/// [`add`] on the `Path` value of `HKEY_CURRENT_USER\<key>`, without telling anyone.
fn add_to(key: &str, folder: &Path, var: impl Fn(&str) -> Option<OsString>) -> io::Result<Added> {
    let key = registry::Key::open(key)?;
    let Some((kind, data)) = key.path()? else {
        key.set_path(
            registry::REG_EXPAND_SZ,
            &terminated(wide(folder.as_os_str())),
        )?;
        return Ok(Added::Created);
    };
    if kind != registry::REG_SZ && kind != registry::REG_EXPAND_SZ {
        return Err(io::Error::other(format!(
            "Path is of registry type {kind}, not text"
        )));
    }
    // The text, without the terminating NUL it is stored with.
    let text = data.strip_suffix(&[0]).unwrap_or(&data);
    if on_path(&String::from_utf16_lossy(text), folder, &var) {
        return Ok(Added::Already);
    }
    let mut value = text.to_vec();
    if !value.is_empty() && value.last() != Some(&u16::from(b';')) {
        value.push(u16::from(b';'));
    }
    value.extend(wide(folder.as_os_str()));
    key.set_path(kind, &terminated(value))?;
    Ok(Added::Appended)
}

/// Whether an entry of `path` names `folder`.
fn on_path(path: &str, folder: &Path, var: &impl Fn(&str) -> Option<OsString>) -> bool {
    let folder = same(&folder.to_string_lossy());
    path.split(';')
        .any(|entry| same(&expand(entry, var)) == folder)
}

/// A folder as Windows compares it: in any case, without quotes or a trailing separator.
fn same(folder: &str) -> String {
    folder
        .trim()
        .trim_matches('"')
        .trim_end_matches(['\\', '/'])
        .to_lowercase()
}

/// `text` with each `%NAME%` that `var` knows replaced by its value; others stay as they are,
/// as `ExpandEnvironmentStrings` leaves them.
fn expand(text: &str, var: &impl Fn(&str) -> Option<OsString>) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('%') else {
            out.push_str(&rest[start..]);
            return out;
        };
        match var(&after[..end]).filter(|_| end > 0) {
            Some(value) => {
                out.push_str(&value.to_string_lossy());
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[start..start + end + 2]);
                rest = &after[end + 1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn wide(text: &std::ffi::OsStr) -> Vec<u16> {
    text.encode_wide().collect()
}

fn terminated(mut text: Vec<u16>) -> Vec<u16> {
    text.push(0);
    text
}

#[allow(unsafe_code)]
mod registry {
    use std::io;

    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_VALUE_TYPE, RegCloseKey,
        RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    };
    pub(super) use windows_sys::Win32::System::Registry::{REG_EXPAND_SZ, REG_SZ};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE,
    };

    /// How long each window may take to read the change.
    const ANNOUNCE_TIMEOUT_MS: u32 = 5000;

    pub(super) fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain([0]).collect()
    }

    /// An open key under `HKEY_CURRENT_USER`, closed when dropped.
    pub(super) struct Key(HKEY);

    impl Key {
        pub(super) fn open(name: &str) -> io::Result<Self> {
            let name = wide(name);
            let mut key: HKEY = std::ptr::null_mut();
            // SAFETY: `name` is NUL-terminated and outlives the call; `key` receives the handle.
            let status = unsafe {
                RegOpenKeyExW(
                    HKEY_CURRENT_USER,
                    name.as_ptr(),
                    0,
                    KEY_QUERY_VALUE | KEY_SET_VALUE,
                    &mut key,
                )
            };
            check(status)?;
            Ok(Self(key))
        }

        /// The `Path` value's type and data as stored (UTF-16, usually NUL-terminated);
        /// `None` when there is none.
        pub(super) fn path(&self) -> io::Result<Option<(REG_VALUE_TYPE, Vec<u16>)>> {
            let name = wide("Path");
            let mut buffer: Vec<u16> = Vec::new();
            loop {
                let mut kind: REG_VALUE_TYPE = 0;
                let mut bytes = u32::try_from(buffer.len() * 2).map_err(io::Error::other)?;
                let data = if buffer.is_empty() {
                    std::ptr::null_mut()
                } else {
                    buffer.as_mut_ptr().cast::<u8>()
                };
                // SAFETY: `data` is null (asking the size) or `bytes` long; `name` is
                // NUL-terminated.
                let status = unsafe {
                    RegQueryValueExW(
                        self.0,
                        name.as_ptr(),
                        std::ptr::null(),
                        &mut kind,
                        data,
                        &mut bytes,
                    )
                };
                match status {
                    ERROR_FILE_NOT_FOUND => return Ok(None),
                    // Asked the size, or the value grew since: room for it, and again.
                    ERROR_SUCCESS if buffer.is_empty() && bytes > 0 => {}
                    ERROR_MORE_DATA => {}
                    ERROR_SUCCESS => {
                        let bytes = bytes as usize;
                        if !bytes.is_multiple_of(2) {
                            return Err(io::Error::other("Path holds an odd number of bytes"));
                        }
                        buffer.truncate(bytes / 2);
                        return Ok(Some((kind, buffer)));
                    }
                    status => return Err(io::Error::from_raw_os_error(status as i32)),
                }
                buffer = vec![0; (bytes as usize).div_ceil(2)];
            }
        }

        pub(super) fn set_path(&self, kind: REG_VALUE_TYPE, data: &[u16]) -> io::Result<()> {
            let name = wide("Path");
            let bytes = u32::try_from(data.len() * 2).map_err(io::Error::other)?;
            // SAFETY: `data` is `bytes` long and `name` NUL-terminated, both outliving the call.
            let status = unsafe {
                RegSetValueExW(self.0, name.as_ptr(), 0, kind, data.as_ptr().cast(), bytes)
            };
            check(status)
        }
    }

    impl Drop for Key {
        fn drop(&mut self) {
            // SAFETY: the handle was opened by `open` and is closed once.
            unsafe { RegCloseKey(self.0) };
        }
    }

    fn check(status: u32) -> io::Result<()> {
        if status == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(status as i32))
        }
    }

    /// Tells every top-level window the user's environment changed, as the Environment
    /// Variables dialog does; a window that doesn't answer is skipped.
    pub(super) fn announce_environment() {
        let area = wide("Environment");
        let mut result = 0usize;
        // SAFETY: `area` is NUL-terminated and outlives the call, which reads it as a string.
        unsafe {
            SendMessageTimeoutW(
                HWND_BROADCAST,
                WM_SETTINGCHANGE,
                0,
                area.as_ptr() as isize,
                SMTO_ABORTIFHUNG,
                ANNOUNCE_TIMEOUT_MS,
                &mut result,
            )
        };
    }
}

#[cfg(test)]
#[allow(unsafe_code)]
mod tests {
    use std::collections::HashMap;

    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_ALL_ACCESS, REG_EXPAND_SZ, REG_OPTION_NON_VOLATILE, REG_SZ,
        RegCloseKey, RegCreateKeyExW, RegDeleteTreeW,
    };

    use super::*;

    /// A scratch `HKCU\Software\BrigadierTest-<random>\Environment`, deleted with its parent
    /// when dropped. The real `HKCU\Environment` is never opened.
    struct Scratch {
        parent: String,
        key: String,
    }

    impl Scratch {
        fn new() -> Self {
            let parent = format!(r"Software\BrigadierTest-{}", uuid::Uuid::new_v4().simple());
            let key = format!(r"{parent}\Environment");
            let name = registry::wide(&key);
            let mut handle: HKEY = std::ptr::null_mut();
            // SAFETY: `name` is NUL-terminated; `handle` receives the key, closed at once.
            let status = unsafe {
                RegCreateKeyExW(
                    HKEY_CURRENT_USER,
                    name.as_ptr(),
                    0,
                    std::ptr::null(),
                    REG_OPTION_NON_VOLATILE,
                    KEY_ALL_ACCESS,
                    std::ptr::null(),
                    &mut handle,
                    std::ptr::null_mut(),
                )
            };
            assert_eq!(status, 0, "the scratch key");
            // SAFETY: opened just above.
            unsafe { RegCloseKey(handle) };
            Self { parent, key }
        }

        fn set(&self, kind: u32, text: &str) {
            let data: Vec<u16> = text.encode_utf16().chain([0]).collect();
            registry::Key::open(&self.key)
                .unwrap()
                .set_path(kind, &data)
                .unwrap();
        }

        fn get(&self) -> Option<(u32, String)> {
            let (kind, data) = registry::Key::open(&self.key).unwrap().path().unwrap()?;
            assert_eq!(data.last(), Some(&0), "stored NUL-terminated");
            Some((kind, String::from_utf16(&data[..data.len() - 1]).unwrap()))
        }

        fn add(&self, folder: &str) -> Added {
            add_to(&self.key, Path::new(folder), vars).unwrap()
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let name = registry::wide(&self.parent);
            // SAFETY: `name` is NUL-terminated; deletes this test's own key and what it holds.
            unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, name.as_ptr()) };
        }
    }

    const FOLDER: &str = r"C:\Users\Me Too\.local\bin";

    fn vars(name: &str) -> Option<OsString> {
        HashMap::from([
            ("USERPROFILE", r"C:\Users\Me Too"),
            ("SystemRoot", r"C:\Windows"),
        ])
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| OsString::from(value))
    }

    #[test]
    fn a_missing_folder_joins_the_end_keeping_the_text_and_its_expandable_type() {
        let scratch = Scratch::new();
        let before =
            r"%SystemRoot%\system32;C:\Tools;%USERPROFILE%\AppData\Local\Microsoft\WindowsApps";
        scratch.set(REG_EXPAND_SZ, before);
        assert_eq!(scratch.add(FOLDER), Added::Appended);
        assert_eq!(
            scratch.get(),
            Some((REG_EXPAND_SZ, format!("{before};{FOLDER}")))
        );
        // There now: not added again.
        assert_eq!(scratch.add(FOLDER), Added::Already);
        assert_eq!(
            scratch.get(),
            Some((REG_EXPAND_SZ, format!("{before};{FOLDER}")))
        );
    }

    #[test]
    fn a_plain_text_path_stays_plain_text_and_a_closing_separator_isnt_doubled() {
        let scratch = Scratch::new();
        scratch.set(REG_SZ, r"C:\Tools;");
        assert_eq!(scratch.add(FOLDER), Added::Appended);
        assert_eq!(scratch.get(), Some((REG_SZ, format!(r"C:\Tools;{FOLDER}"))));
    }

    #[test]
    fn the_folder_already_there_in_any_spelling_is_not_added_again() {
        for there in [
            r"c:\users\me too\.LOCAL\BIN",
            r"C:\Users\Me Too\.local\bin\",
            r"%USERPROFILE%\.local\bin",
            r"%userprofile%\.local\bin\",
            r#""C:\Users\Me Too\.local\bin""#,
        ] {
            for kind in [REG_EXPAND_SZ, REG_SZ] {
                let scratch = Scratch::new();
                let before = format!(r"C:\Tools;{there};C:\Other");
                scratch.set(kind, &before);
                assert_eq!(scratch.add(FOLDER), Added::Already, "{there}");
                assert_eq!(scratch.get(), Some((kind, before)), "{there}");
            }
        }
    }

    #[test]
    fn no_path_value_is_created_expandable_and_an_empty_one_gets_just_the_folder() {
        let scratch = Scratch::new();
        assert_eq!(scratch.get(), None);
        assert_eq!(scratch.add(FOLDER), Added::Created);
        assert_eq!(scratch.get(), Some((REG_EXPAND_SZ, FOLDER.to_owned())));

        let scratch = Scratch::new();
        scratch.set(REG_EXPAND_SZ, "");
        assert_eq!(scratch.add(FOLDER), Added::Appended);
        assert_eq!(scratch.get(), Some((REG_EXPAND_SZ, FOLDER.to_owned())));
    }

    #[test]
    fn a_long_path_is_read_whole() {
        let scratch = Scratch::new();
        let before = (0..400)
            .map(|i| format!(r"C:\Folder{i}"))
            .collect::<Vec<_>>()
            .join(";");
        scratch.set(REG_EXPAND_SZ, &before);
        assert_eq!(scratch.add(FOLDER), Added::Appended);
        assert_eq!(
            scratch.get(),
            Some((REG_EXPAND_SZ, format!("{before};{FOLDER}")))
        );
    }

    #[test]
    fn unknown_variables_and_lone_percent_signs_stay_as_written() {
        assert_eq!(
            expand(r"%NOPE%\x;%SystemRoot%\y", &vars),
            r"%NOPE%\x;C:\Windows\y"
        );
        assert_eq!(expand("50%", &vars), "50%");
        assert_eq!(expand(r"%systemroot%\z", &vars), r"C:\Windows\z");
    }
}
