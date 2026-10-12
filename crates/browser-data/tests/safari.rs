//! Safari cookies (binarycookies) and history from generated fixtures.
// The fixtures set Unix file modes; Safari exists only on macOS anyway.
#![cfg(unix)]

mod common;

use std::os::unix::fs::PermissionsExt;

use brigadier_browser_data::{
    Error, Family, count_cookies, discover_in, read_cookies, read_history,
};
use common::*;

fn safari_home() -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    mkdirs(&home.path().join("Library/Safari"));
    home
}

fn cookies_path(home: &std::path::Path) -> std::path::PathBuf {
    home.join("Library/Containers/com.apple.Safari/Data/Library/Cookies/Cookies.binarycookies")
}

#[test]
fn binarycookies_flags_domains_and_expiry() {
    let home = safari_home();
    let future = apple_time(NOW + 3600);
    let file = encode_binarycookies(&[
        vec![
            BinCookie {
                domain: ".apple.example",
                name: "both",
                path: "/",
                value: "1",
                flags: 5,
                expiry: future,
            },
            BinCookie {
                domain: "www.apple.example",
                name: "secure",
                path: "/s",
                value: "2",
                flags: 1,
                expiry: future + 0.75,
            },
        ],
        vec![
            BinCookie {
                domain: "apple.example",
                name: "http_only",
                path: "/",
                value: "ünïcode",
                flags: 4,
                expiry: future,
            },
            BinCookie {
                domain: ".apple.example",
                name: "expired",
                path: "/",
                value: "x",
                flags: 0,
                expiry: apple_time(NOW - 1),
            },
        ],
    ]);
    let path = cookies_path(home.path());
    mkdirs(path.parent().unwrap());
    std::fs::write(&path, file).unwrap();

    let profiles = discover_in(home.path(), &[]);
    let safari = profiles.iter().find(|p| p.id == "safari/default").unwrap();
    assert!(safari.has_cookies && safari.readable && !safari.has_history);

    let out = read_cookies(safari, None, NOW).unwrap();
    assert_eq!((out.expired, out.partitioned, out.unreadable), (1, 0, 0));
    let got: Vec<_> = out
        .cookies
        .iter()
        .map(|c| {
            (
                c.name.as_str(),
                c.domain.as_str(),
                c.host_only,
                c.secure,
                c.http_only,
                c.path.as_str(),
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            ("both", ".apple.example", false, true, true, "/"),
            ("secure", "www.apple.example", true, true, false, "/s"),
            ("http_only", "apple.example", true, false, true, "/"),
        ]
    );
    assert_eq!(out.cookies[2].value, "ünïcode");
    assert!(
        out.cookies
            .iter()
            .all(|c| c.expires_unix == Some(NOW + 3600))
    );
    assert!(out.cookies.iter().all(|c| c.same_site.is_none()));
    // The count takes every record, expired ones too.
    assert_eq!(count_cookies(safari).unwrap(), 4);
}

#[test]
fn a_truncated_record_is_unreadable_not_a_panic() {
    let home = safari_home();
    let mut file = encode_binarycookies(&[vec![BinCookie {
        domain: ".a.example",
        name: "n",
        path: "/",
        value: "v",
        flags: 0,
        expiry: apple_time(NOW + 10),
    }]]);
    // Make the record claim a size far beyond its page.
    let record_start = 4 + 4 + 4 + 8 + 4 + 4;
    file[record_start..record_start + 4].copy_from_slice(&10_000u32.to_le_bytes());
    let path = cookies_path(home.path());
    mkdirs(path.parent().unwrap());
    std::fs::write(&path, file).unwrap();
    let safari = profile(
        "safari/default",
        Family::Safari,
        home.path().join("Library/Safari"),
    );
    let out = read_cookies(&safari, None, NOW).unwrap();
    assert_eq!((out.cookies.len(), out.unreadable), (0, 1));
}

#[test]
fn history_db_times_and_titles() {
    let home = safari_home();
    let conn = safari_history_db(&home.path().join("Library/Safari/History.db"));
    conn.execute_batch(
        "INSERT INTO history_items (id, url) VALUES
           (1, 'https://webkit.example/'), (2, 'http://old.example/'), (3, 'file:///tmp/x');",
    )
    .unwrap();
    for (item, secs, title) in [
        (1, apple_time(NOW) + 0.25, Some("WebKit")),
        (1, apple_time(NOW) + 0.25, Some("WebKit")),
        (2, apple_time(NOW - 30), None),
        (3, apple_time(NOW + 5), Some("local")),
    ] {
        conn.execute(
            "INSERT INTO history_visits (history_item, visit_time, title) VALUES (?1, ?2, ?3)",
            rusqlite::params![item, secs, title],
        )
        .unwrap();
    }
    drop(conn);
    let safari = profile(
        "safari/default",
        Family::Safari,
        home.path().join("Library/Safari"),
    );
    let visits = read_history(&safari, 10).unwrap();
    let got: Vec<_> = visits
        .iter()
        .map(|v| (v.url.as_str(), v.title.as_str(), v.visited_at_ms))
        .collect();
    assert_eq!(
        got,
        [
            ("https://webkit.example/", "WebKit", NOW * 1000 + 250),
            ("http://old.example/", "", (NOW - 30) * 1000),
        ]
    );
}

#[test]
fn an_unopenable_cookie_file_is_permission_denied() {
    let home = safari_home();
    let path = cookies_path(home.path());
    mkdirs(path.parent().unwrap());
    std::fs::write(&path, encode_binarycookies(&[])).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::File::open(&path).is_ok() {
        // Running as root: permissions don't apply.
        eprintln!("skipped: file permissions are not enforced for this user");
        return;
    }
    let profiles = discover_in(home.path(), &[]);
    let safari = profiles.iter().find(|p| p.id == "safari/default").unwrap();
    assert!(safari.has_cookies);
    assert!(!safari.readable);
    assert!(matches!(
        read_cookies(safari, None, NOW),
        Err(Error::PermissionDenied(_))
    ));
    assert!(matches!(
        count_cookies(safari),
        Err(Error::PermissionDenied(_))
    ));
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

#[test]
fn an_unopenable_history_db_is_permission_denied() {
    let home = safari_home();
    let path = home.path().join("Library/Safari/History.db");
    drop(safari_history_db(&path));
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::File::open(&path).is_ok() {
        eprintln!("skipped: file permissions are not enforced for this user");
        return;
    }
    let safari = profile(
        "safari/default",
        Family::Safari,
        home.path().join("Library/Safari"),
    );
    assert!(matches!(
        read_history(&safari, 10),
        Err(Error::PermissionDenied(_))
    ));
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
}
