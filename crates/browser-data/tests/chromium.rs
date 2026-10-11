//! Chromium cookies and history from generated fixtures.

mod common;

use brigadier_browser_data::{Family, SameSite, count_cookies, read_cookies, read_history};
use common::*;

fn by_name<'a>(
    cookies: &'a brigadier_browser_data::Cookies,
    name: &str,
) -> &'a brigadier_browser_data::Cookie {
    cookies
        .cookies
        .iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("no cookie {name}"))
}

/// Fills a store with one of each interesting case. `hash` decides whether values carry the
/// domain hash (version 24+).
fn fill(conn: &rusqlite::Connection, hash: bool) {
    let enc = |value: &str, host: &str| chromium_encrypt(PASSWORD, value, hash.then_some(host));
    let rows = [
        ChromiumRow {
            name: "domain",
            encrypted: enc("sécret-1", ".example.com"),
            secure: true,
            http_only: true,
            samesite: 2,
            ..Default::default()
        },
        ChromiumRow {
            host: "app.example.com",
            name: "host_only",
            encrypted: enc("v2", "app.example.com"),
            path: "/app",
            samesite: 1,
            ..Default::default()
        },
        ChromiumRow {
            name: "plain",
            value: "plain-value",
            samesite: 0,
            ..Default::default()
        },
        ChromiumRow {
            name: "session",
            encrypted: enc("s", ".example.com"),
            persistent: false,
            ..Default::default()
        },
        ChromiumRow {
            name: "expired",
            encrypted: enc("old", ".example.com"),
            expires_unix: NOW - 10,
            ..Default::default()
        },
        ChromiumRow {
            name: "partitioned",
            top_frame: "https://top.example",
            encrypted: enc("p", ".example.com"),
            ..Default::default()
        },
    ];
    for row in &rows {
        insert_chromium(conn, row);
    }
}

#[test]
fn version_23_values_decrypt_without_a_domain_hash() {
    let dir = tempfile::tempdir().unwrap();
    let conn = chromium_cookies_db(&dir.path().join("Cookies"), 23);
    fill(&conn, false);
    drop(conn);
    let profile = profile("t/Default", Family::Chromium, dir.path().to_owned());

    let out = read_cookies(&profile, Some(PASSWORD), NOW).unwrap();
    assert_eq!((out.expired, out.partitioned, out.unreadable), (1, 1, 0));
    assert_eq!(out.cookies.len(), 4);

    let domain = by_name(&out, "domain");
    assert_eq!(domain.value, "sécret-1");
    assert_eq!(domain.domain, ".example.com");
    assert!(!domain.host_only && domain.secure && domain.http_only);
    assert_eq!(domain.same_site, Some(SameSite::Strict));
    assert_eq!(domain.expires_unix, Some(NOW + 86_400));

    let host_only = by_name(&out, "host_only");
    assert_eq!(host_only.value, "v2");
    assert_eq!(host_only.domain, "app.example.com");
    assert!(host_only.host_only && !host_only.secure && !host_only.http_only);
    assert_eq!(host_only.path, "/app");
    assert_eq!(host_only.same_site, Some(SameSite::Lax));

    let plain = by_name(&out, "plain");
    assert_eq!(plain.value, "plain-value");
    assert_eq!(plain.same_site, Some(SameSite::None));

    let session = by_name(&out, "session");
    assert_eq!(session.expires_unix, None);
    assert_eq!(session.same_site, None);
}

#[test]
fn version_24_values_carry_and_check_the_domain_hash() {
    let dir = tempfile::tempdir().unwrap();
    let conn = chromium_cookies_db(&dir.path().join("Network/Cookies"), 24);
    fill(&conn, true);
    // A record moved to another domain: its hash is of a different host.
    insert_chromium(
        &conn,
        &ChromiumRow {
            host: ".evil.example",
            name: "moved",
            encrypted: chromium_encrypt(PASSWORD, "stolen", Some(".example.com")),
            ..Default::default()
        },
    );
    drop(conn);
    let profile = profile("t/Default", Family::Chromium, dir.path().to_owned());

    let out = read_cookies(&profile, Some(PASSWORD), NOW).unwrap();
    assert_eq!((out.expired, out.partitioned, out.unreadable), (1, 1, 1));
    assert_eq!(by_name(&out, "domain").value, "sécret-1");
    assert_eq!(by_name(&out, "host_only").value, "v2");
    assert_eq!(by_name(&out, "session").value, "s");
    assert!(out.cookies.iter().all(|c| c.name != "moved"));
}

#[test]
fn wrong_or_missing_password_counts_encrypted_cookies_unreadable() {
    let dir = tempfile::tempdir().unwrap();
    let conn = chromium_cookies_db(&dir.path().join("Cookies"), 24);
    fill(&conn, true);
    drop(conn);
    let profile = profile("t/Default", Family::Chromium, dir.path().to_owned());

    for password in [Some(b"wrong-password".as_slice()), None] {
        let out = read_cookies(&profile, password, NOW).unwrap();
        // Only the plaintext cookie survives; the expired and partitioned ones are counted
        // before decryption is attempted.
        assert_eq!(out.cookies.len(), 1, "{password:?}");
        assert_eq!(out.cookies[0].name, "plain");
        assert_eq!((out.expired, out.partitioned, out.unreadable), (1, 1, 3));
        assert_eq!(
            out.unreadable_domains,
            [".example.com", "app.example.com", ".example.com"]
        );
    }
}

#[test]
fn counts_every_cookie_without_a_password_even_mid_write() {
    let dir = tempfile::tempdir().unwrap();
    let conn = chromium_cookies_db(&dir.path().join("Network/Cookies"), 24);
    conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA wal_autocheckpoint = 0;")
        .unwrap();
    fill(&conn, true);
    let open = profile("t/Default", Family::Chromium, dir.path().to_owned());
    // `conn` stays open, so the rows live only in Cookies-wal: they are counted all the same.
    assert_eq!(count_cookies(&open).unwrap(), 6);
    drop(conn);
    assert!(matches!(
        count_cookies(&profile(
            "t/Gone",
            Family::Chromium,
            dir.path().join("gone")
        )),
        Err(brigadier_browser_data::Error::NotFound(_))
    ));
}

#[test]
fn older_column_names_are_understood() {
    let dir = tempfile::tempdir().unwrap();
    let conn = rusqlite::Connection::open(dir.path().join("Cookies")).unwrap();
    conn.execute_batch(
        "CREATE TABLE meta(key LONGVARCHAR NOT NULL UNIQUE PRIMARY KEY, value LONGVARCHAR);
         INSERT INTO meta VALUES ('version', '9');
         CREATE TABLE cookies(creation_utc INTEGER NOT NULL, host_key TEXT NOT NULL,
           name TEXT NOT NULL, value TEXT NOT NULL, path TEXT NOT NULL,
           expires_utc INTEGER NOT NULL, secure INTEGER NOT NULL, httponly INTEGER NOT NULL,
           last_access_utc INTEGER NOT NULL, has_expires INTEGER NOT NULL,
           encrypted_value BLOB DEFAULT '');",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO cookies VALUES (0, 'old.example', 'a', '', '/', ?1, 1, 1, 0, 1, ?2)",
        rusqlite::params![
            chromium_time(NOW + 60),
            chromium_encrypt(PASSWORD, "b", None)
        ],
    )
    .unwrap();
    drop(conn);
    let profile = profile("t/Default", Family::Chromium, dir.path().to_owned());
    let out = read_cookies(&profile, Some(PASSWORD), NOW).unwrap();
    assert_eq!(out.cookies.len(), 1);
    let c = &out.cookies[0];
    assert_eq!((c.value.as_str(), c.secure, c.http_only), ("b", true, true));
    assert_eq!(c.expires_unix, Some(NOW + 60));
    assert!(c.host_only);
}

#[test]
fn history_is_newest_first_one_per_visit_web_only() {
    let dir = tempfile::tempdir().unwrap();
    let conn = chromium_history_db(&dir.path().join("History"));
    conn.execute_batch(
        "INSERT INTO urls (id, url, title, last_visit_time) VALUES
           (1, 'https://a.example/', 'A', 0),
           (2, 'http://b.example/x', NULL, 0),
           (3, 'chrome://settings/', 'Settings', 0),
           (4, 'file:///etc/hosts', 'hosts', 0);",
    )
    .unwrap();
    // Exact microseconds: 2026-10-11T00:00:00.123456Z etc.
    let t = |unix: i64, micros: i64| chromium_time(unix) + micros;
    for (url, time) in [
        (1, t(NOW, 123_456)),
        (1, t(NOW, 123_456)), // a duplicate row of the same visit
        (1, t(NOW - 3600, 0)),
        (2, t(NOW + 10, 999)),
        (3, t(NOW + 20, 0)),
        (4, t(NOW + 30, 0)),
    ] {
        conn.execute(
            "INSERT INTO visits (url, visit_time) VALUES (?1, ?2)",
            [url, time],
        )
        .unwrap();
    }
    drop(conn);
    let profile = profile("t/Default", Family::Chromium, dir.path().to_owned());

    let visits = read_history(&profile, 100).unwrap();
    let got: Vec<_> = visits
        .iter()
        .map(|v| (v.url.as_str(), v.title.as_str(), v.visited_at_ms))
        .collect();
    assert_eq!(
        got,
        [
            ("http://b.example/x", "", (NOW + 10) * 1000),
            ("https://a.example/", "A", NOW * 1000 + 123),
            ("https://a.example/", "A", (NOW - 3600) * 1000),
        ]
    );
    assert_eq!(read_history(&profile, 2).unwrap().len(), 2);
}
