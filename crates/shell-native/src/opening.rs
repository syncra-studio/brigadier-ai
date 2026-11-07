//! What the shell hands to the system's default app, and what it only shows in the file
//! manager (#770). A worker names the files it writes, and a model writes the paths an answer
//! links, so neither may launch a program: a file the default app would run (an app or
//! bundle, installer, script, shortcut, web or launcher link: [`LAUNCH_TYPES`], shared with
//! the app's attachment rows) is shown in the file manager instead, and anything that isn't a
//! file or folder on disk (a URI of any scheme) is refused.

use std::collections::HashSet;
use std::path::Path;
use std::sync::LazyLock;

/// The file types the default app would run, one list for the app and the shell.
const LAUNCH_TYPES: &str = include_str!("../../../apps/desktop/src/lib/launchTypes.json");

static LAUNCHES: LazyLock<HashSet<String>> = LazyLock::new(|| {
    #[derive(serde::Deserialize)]
    struct Types {
        launches: Vec<String>,
        scripts: Vec<String>,
    }
    let types: Types = serde_json::from_str(LAUNCH_TYPES).expect("launchTypes.json");
    types.launches.into_iter().chain(types.scripts).collect()
});

/// What to do with a path asked to open.
#[derive(Debug, PartialEq, Eq)]
pub enum Opening {
    /// Open it with the default app (or the app named).
    Open,
    /// Show it in the file manager: opened, it would run something. Its name, as the user
    /// would read it.
    Reveal(String),
    /// Neither: why.
    Refuse(String),
}

/// What is at a path, as far as opening it goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Found {
    Missing,
    Directory,
    /// A file; `executable`: it has an execute bit (Unix), so the default app runs it.
    File {
        executable: bool,
    },
}

/// The system whose rules apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum System {
    Windows,
    Unix,
}

/// Whether to open `target` (a path the page gives) with the default app, or with an app
/// named when `with_app`, given what is there (`found`) and the name it resolves to through
/// links (`resolved`), on `system`, whose `PATHEXT` (Windows) is `pathext`.
pub fn opening(
    target: &str,
    found: Found,
    resolved: Option<&str>,
    with_app: bool,
    system: System,
    pathext: &str,
) -> Opening {
    if has_scheme(target) {
        return Opening::Refuse(format!("only a file or folder opens here: {target}"));
    }
    let plain = match system {
        System::Windows => without_verbatim(target),
        System::Unix => target.into(),
    };
    if !absolute(&plain, system) || (system == System::Windows && stream_or_device(&plain)) {
        return Opening::Refuse(format!("not a file or folder path: {target}"));
    }
    if found == Found::Missing {
        return Opening::Refuse(format!("{target} isn't there"));
    }
    let name = file_name(&plain);
    // A bundle (`Name.app`) is a folder the file manager would run: its type decides first.
    let launches = [Some(plain.as_str()), resolved]
        .into_iter()
        .flatten()
        .any(|path| launches(file_name(path), system, pathext));
    if launches || found == (Found::File { executable: true }) {
        return Opening::Reveal(name.to_owned());
    }
    if with_app && found != Found::Directory {
        return Opening::Refuse(format!("only a folder opens in another app: {target}"));
    }
    Opening::Open
}

/// Whether a file named `name` runs something when the default app opens it.
pub fn launches(name: &str, system: System, pathext: &str) -> bool {
    // Windows drops trailing dots and spaces: `run.bat.` is `run.bat`.
    let bare = name.trim_end_matches(['.', ' ']);
    let Some((_, extension)) = bare.rsplit_once('.') else {
        return false;
    };
    let extension = extension.to_ascii_lowercase();
    if LAUNCHES.contains(&extension) {
        return true;
    }
    system == System::Windows
        && pathext
            .split(';')
            .filter_map(|ext| ext.trim().strip_prefix('.'))
            .any(|ext| ext.eq_ignore_ascii_case(&extension))
}

/// Whether `target` starts with a URI scheme (`file:`, `ms-settings:`, `x-handler:`). A
/// drive letter (`C:`) is none.
fn has_scheme(target: &str) -> bool {
    let Some((scheme, _)) = target.split_once(':') else {
        return false;
    };
    let mut chars = scheme.chars();
    let starts = chars.next().is_some_and(|c| c.is_ascii_alphabetic());
    let rest = chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    starts && rest && scheme.len() > 1
}

/// Whether `target` is an absolute path on `system`: from a drive's root (`C:\`, `C:/`) or a
/// share (`\\server\share`) on Windows, from `/` elsewhere.
fn absolute(target: &str, system: System) -> bool {
    match system {
        System::Unix => target.starts_with('/'),
        System::Windows => {
            let bytes = target.as_bytes();
            let drive = bytes.len() >= 3
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && matches!(bytes[2], b'\\' | b'/');
            drive || target.starts_with(r"\\") || target.starts_with("//")
        }
    }
}

/// A Windows verbatim path to a drive's file or a share's (`\\?\C:\x`, `\\?\UNC\server\share\x`:
/// what canonicalizing gives, as a project's folder is stored) as the plain path it names;
/// any other path as it is, so another `\\?\` path is still a device path.
fn without_verbatim(target: &str) -> String {
    if let Some(share) = target.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{share}");
    }
    match target.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => rest.into(),
        _ => target.into(),
    }
}

/// A Windows path that names a file's stream (`a.txt:x.exe`) or a device namespace
/// (`\\?\`, `\\.\`), not a plain file or folder.
fn stream_or_device(target: &str) -> bool {
    let after_drive = target.get(2..).unwrap_or("");
    after_drive.contains(':')
        || ["\\\\?\\", "\\\\.\\", "//?/", "//./"]
            .iter()
            .any(|prefix| target.starts_with(prefix))
}

/// The last part of `path`, by either separator.
fn file_name(path: &str) -> &str {
    let trimmed = path.trim_end_matches(['/', '\\']);
    trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed)
}

/// [`opening`] for `target` on this system, looking at what is on disk.
pub fn decide(target: &str, with_app: bool) -> Opening {
    let path = Path::new(target);
    let found = match std::fs::metadata(path) {
        Err(_) => Found::Missing,
        Ok(meta) if meta.is_dir() => Found::Directory,
        Ok(meta) => Found::File {
            executable: executable(&meta),
        },
    };
    let resolved = std::fs::canonicalize(path)
        .ok()
        .and_then(|path| path.to_str().map(str::to_owned));
    let (system, pathext) = if cfg!(windows) {
        (
            System::Windows,
            std::env::var("PATHEXT").unwrap_or_default(),
        )
    } else {
        (System::Unix, String::new())
    };
    opening(
        target,
        found,
        resolved.as_deref(),
        with_app,
        system,
        &pathext,
    )
}

#[cfg(unix)]
fn executable(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn executable(_meta: &std::fs::Metadata) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: Found = Found::File { executable: false };
    const PATHEXT: &str = ".COM;.EXE;.BAT;.CMD;.VBS;.VBE;.JS;.JSE;.WSF;.WSH;.MSC;.CPL;.PY";

    fn windows(target: &str, found: Found) -> Opening {
        opening(target, found, None, false, System::Windows, PATHEXT)
    }

    fn mac(target: &str, found: Found) -> Opening {
        opening(target, found, None, false, System::Unix, "")
    }

    fn revealed(opening: &Opening) -> bool {
        matches!(opening, Opening::Reveal(_))
    }

    fn refused(opening: &Opening) -> bool {
        matches!(opening, Opening::Refuse(_))
    }

    /// A worker's file of a type that runs is shown in the file manager, never opened: the
    /// cases #770 names, any case, and Windows' dropped trailing dots and spaces.
    #[test]
    fn a_file_that_would_run_is_revealed_not_opened() {
        for name in [
            "report.bat",
            "REPORT.CMD",
            "x.ps1",
            "x.lnk",
            "x.url",
            "x.hta",
            "x.js",
            "x.vbs",
            "setup.exe",
            "setup.msi",
            "x.scr",
            "x.reg",
            "x.jar",
            "x.appref-ms",
            "x.settingcontent-ms",
            "x.library-ms",
            "x.chm",
            "x.iso",
            "run.bat.",
            "run.bat ",
            "run.bat . .",
            "x.psm1",
            "x.msc",
            "update.msu",
            "box.wsb",
            "ext.vsix",
            "dark.theme",
            "x.appcontent-ms",
        ] {
            let target = format!(r"C:\Users\me\AppData\Local\Brigadier\cache\open\abc\{name}");
            assert_eq!(
                windows(&target, FILE),
                Opening::Reveal(name.into()),
                "{name}"
            );
        }
        for name in [
            "report.terminal",
            "run.command",
            "a.tool",
            "x.webloc",
            "x.scpt",
            "x.sh",
        ] {
            let target = format!("/Users/me/Library/Brigadier/cache/open/abc/{name}");
            assert!(revealed(&mac(&target, FILE)), "{name}");
        }
    }

    /// Documents, images and data open in their app as before.
    #[test]
    fn documents_open_in_their_app() {
        for name in [
            "report.pdf",
            "notes.md",
            "data.csv",
            "deck.pptx",
            "photo.PNG",
            "page.html",
            "README",
        ] {
            let target = format!(r"C:\work\{name}");
            assert_eq!(windows(&target, FILE), Opening::Open, "{name}");
            let target = format!("/work/{name}");
            assert_eq!(mac(&target, FILE), Opening::Open, "{name}");
        }
        assert_eq!(windows("C:/work/out/report.docx", FILE), Opening::Open);
        assert_eq!(windows(r"\\server\share\report.docx", FILE), Opening::Open);
    }

    /// `PATHEXT` adds the types this Windows runs, beyond the shared list.
    #[test]
    fn windows_pathext_counts() {
        let target = r"C:\work\tool.cobra";
        assert_eq!(windows(target, FILE), Opening::Open);
        assert!(revealed(&opening(
            target,
            FILE,
            None,
            false,
            System::Windows,
            ".EXE;.COBRA"
        )));
        // Elsewhere there is no PATHEXT.
        assert!(!launches("tool.cobra", System::Unix, ".COBRA"));
    }

    /// Anything with a URI scheme is refused, whatever its type, before the type is looked
    /// at; a drive letter is no scheme. These are what a model's file link may hold, which the
    /// markdown link's fallback passes on as they are.
    #[test]
    fn a_uri_is_refused_and_a_drive_letter_is_not_a_scheme() {
        for target in [
            "ms-settings:display",
            "file:///C:/Users/me/report.pdf",
            "file:report.bat",
            "http://example.com/report.pdf",
            "https://example.com/",
            "x-handler:run",
            "ms-msdt:/id",
            "search-ms:query=x",
            "mailto:me@example.com",
            "vscode://file/C:/x",
        ] {
            assert!(refused(&windows(target, FILE)), "{target}");
            assert!(refused(&windows(target, Found::Directory)), "{target}");
            assert!(refused(&mac(target, FILE)), "{target}");
        }
        assert!(!has_scheme(r"C:\work\report.pdf"));
        assert!(!has_scheme("c:/work/report.pdf"));
        assert!(!has_scheme("/Users/me/a:b.md"));
        assert_eq!(windows(r"C:\work\report.pdf", FILE), Opening::Open);
    }

    /// A relative path, a drive-relative one, a file's stream and a device path are not
    /// files or folders to open.
    #[test]
    fn only_a_plain_absolute_path_opens() {
        for target in [
            r"report.pdf",
            r"work\report.pdf",
            r"C:report.pdf",
            r"C:\work\notes.txt:payload.exe",
            r"C:\work\notes.txt:Zone.Identifier",
            r"\\?\C:\work\notes.txt:payload.exe",
            r"\\?\UNC\server\share\notes.txt:payload.exe",
            r"\\?\C:",
            r"\\?\GLOBALROOT\Device\HarddiskVolume1\report.pdf",
            r"\\?\Volume{00000000-0000-0000-0000-000000000000}\report.pdf",
            r"\\.\pipe\brigadier",
            r"\\.\C:\work\report.pdf",
            "//?/C:/work/report.pdf",
            "/Users/me/report.pdf",
        ] {
            assert!(refused(&windows(target, FILE)), "{target}");
        }
        assert!(refused(&mac("work/report.pdf", FILE)));
        assert!(refused(&windows(r"C:\work\gone.pdf", Found::Missing)));
    }

    /// A canonical Windows path (`\\?\C:\…`, as a project's folder is stored) is the plain path
    /// it names: a folder or document opens, a launcher is still only shown.
    #[test]
    fn a_canonical_windows_path_is_a_plain_one() {
        assert_eq!(
            windows(r"\\?\C:\Development\repo", Found::Directory),
            Opening::Open
        );
        assert_eq!(windows(r"\\?\C:\work\report.pdf", FILE), Opening::Open);
        assert_eq!(
            windows(r"\\?\UNC\server\share\report.docx", FILE),
            Opening::Open
        );
        assert_eq!(
            windows(r"\\?\C:\work\setup.exe", FILE),
            Opening::Reveal("setup.exe".into())
        );
        assert!(revealed(&windows(r"\\?\UNC\server\share\run.bat", FILE)));
    }

    /// A bundle is a folder the file manager runs: its type wins over its being a folder.
    #[test]
    fn a_bundle_folder_is_revealed_not_opened() {
        for target in [
            "/Users/me/out/Evil.app",
            "/Users/me/out/Evil.app/",
            "/x/a.workflow",
            "/x/b.pkg",
        ] {
            assert!(revealed(&mac(target, Found::Directory)), "{target}");
        }
        assert!(revealed(&windows(r"C:\out\Evil.app", Found::Directory)));
        assert_eq!(mac("/Users/me/project", Found::Directory), Opening::Open);
        assert_eq!(
            windows(r"C:\Users\me\project", Found::Directory),
            Opening::Open
        );
    }

    /// A file with an execute bit runs when opened, whatever its name; and a link is judged
    /// by what it points at too.
    #[test]
    fn an_executable_or_a_link_to_one_is_revealed() {
        assert!(revealed(&mac(
            "/work/gradlew",
            Found::File { executable: true }
        )));
        let link = opening(
            r"C:\work\notes.txt",
            FILE,
            Some(r"C:\work\payload.exe"),
            false,
            System::Windows,
            PATHEXT,
        );
        assert!(revealed(&link));
    }

    /// Opening with an app named (Terminal) takes folders only: a file there would run.
    #[test]
    fn another_app_opens_folders_only() {
        let with = |target: &str, found| opening(target, found, None, true, System::Unix, "");
        assert_eq!(with("/Users/me/project", Found::Directory), Opening::Open);
        assert!(refused(&with("/Users/me/project/notes.md", FILE)));
        assert!(revealed(&with("/Users/me/project/run.sh", FILE)));
    }

    /// On a real Windows disk: a launcher named by its 8.3 short name (`PAYLOA~1.SET` for
    /// `payload.settingcontent-ms`) is judged by its long name; a name with a trailing dot is
    /// the file it opens; a folder named like a program is only shown; a project folder in its
    /// canonical form opens.
    #[cfg(windows)]
    #[test]
    fn on_disk_a_short_or_dotted_name_is_judged_by_the_file_it_opens() {
        let dir =
            std::env::temp_dir().join(format!("win-issues-w16-opening-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("x.exe")).unwrap();
        std::fs::write(dir.join("payload.settingcontent-ms"), "<x/>").unwrap();
        std::fs::write(dir.join("run.bat"), "@echo off").unwrap();
        std::fs::write(dir.join("notes.txt"), "notes").unwrap();
        let at = |name: &str| format!(r"{}\{name}", dir.display());

        // 8.3 names may be off on this volume; the long name is checked then.
        let short = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .find(|entry| entry.file_name() == "payload.settingcontent-ms")
            .map(|_| at("PAYLOA~1.SET"))
            .filter(|short| std::fs::metadata(short).is_ok())
            .unwrap_or_else(|| at("payload.settingcontent-ms"));
        let outcomes = [
            (decide(&short, false), "short name"),
            (decide(&at("run.bat."), false), "trailing dot"),
            (decide(&at("x.exe"), false), "folder named like a program"),
        ];
        let canonical = std::fs::canonicalize(&dir).unwrap();
        let opens = decide(canonical.to_str().unwrap(), false);
        let document = decide(&at("notes.txt"), false);
        std::fs::remove_dir_all(&dir).unwrap();
        for (outcome, case) in outcomes {
            assert!(revealed(&outcome), "{case}: {outcome:?}");
        }
        assert_eq!(opens, Opening::Open);
        assert_eq!(document, Opening::Open);
    }

    /// The list the app reads is the list the shell reads.
    #[test]
    fn the_shared_list_holds_both_kinds() {
        for extension in [
            "exe", "app", "terminal", "lnk", "url", "bat", "ps1", "js", "sh",
        ] {
            assert!(LAUNCHES.contains(extension), "{extension}");
        }
    }
}
