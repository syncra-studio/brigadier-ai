//! Firefox-family cookies and history from generated fixtures.

mod common;

use brigadier_browser_data::{Cookies, Family, SameSite, read_cookies, read_history};
use common::*;

/// name, host, path, expiry, secure, httponly, sameSite, originAttributes
type Row<'a> = (&'a str, &'a str, &'a str, i64, i64, i64, i64, &'a str);

/// Inserts the same cookies into a store, with expiry scaled by `unit` (1 = seconds,
/// 1000 = milliseconds).
fn fill(conn: &rusqlite::Connection, unit: i64, partitioned_column: bool) {
    let expiry = (NOW + 86_400) * unit;
    let rows: [Row<'_>; 6] = [
        ("domain", ".example.com", "/", expiry, 1, 1, 2, ""),
        ("host", "app.example.com", "/app", expiry, 0, 0, 1, ""),
        ("none", "example.com", "/", expiry, 1, 0, 0, ""),
        ("expired", "example.com", "/", (NOW - 5) * unit, 0, 0, 0, ""),
        (
            "container",
            "example.com",
            "/",
            expiry,
            0,
            0,
            0,
            "^userContextId=2",
        ),
        (
            "chips",
            "example.com",
            "/",
            expiry,
            1,
            0,
            0,
            "^partitionKey=%28https%2Ctop.example%29",
        ),
    ];
    for (name, host, path, expiry, secure, http_only, same_site, origin) in rows {
        conn.execute(
            "INSERT INTO moz_cookies (originAttributes, name, value, host, path, expiry,
               lastAccessed, creationTime, isSecure, isHttpOnly, sameSite, rawSameSite)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, 0, ?7, ?8, ?9, ?9)",
            rusqlite::params![
                origin,
                name,
                format!("{name}-value"),
                host,
                path,
                expiry,
                secure,
                http_only,
                same_site
            ],
        )
        .unwrap();
    }
    if partitioned_column {
        conn.execute(
            "INSERT INTO moz_cookies (name, value, host, path, expiry, isSecure, isHttpOnly,
               isPartitionedAttributeSet) VALUES ('flagged', 'x', 'example.com', '/', ?1, 1, 0, 1)",
            [expiry],
        )
        .unwrap();
    }
}

fn check(out: &Cookies, flagged: usize) {
    assert_eq!(
        (out.expired, out.partitioned, out.unreadable),
        (1, 2 + flagged, 0)
    );
    let names: Vec<_> = out.cookies.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["domain", "host", "none"]);
    let [domain, host, none] = &out.cookies[..] else {
        unreachable!()
    };
    assert_eq!(domain.value, "domain-value");
    assert!(!domain.host_only && domain.secure && domain.http_only);
    assert_eq!(domain.same_site, Some(SameSite::Strict));
    assert!(host.host_only && !host.secure && !host.http_only);
    assert_eq!(
        (host.domain.as_str(), host.path.as_str()),
        ("app.example.com", "/app")
    );
    assert_eq!(host.same_site, Some(SameSite::Lax));
    assert_eq!(none.same_site, Some(SameSite::None));
    for cookie in &out.cookies {
        assert_eq!(cookie.expires_unix, Some(NOW + 86_400), "{}", cookie.name);
    }
}

#[test]
fn old_schema_seconds_and_new_schema_milliseconds_give_the_same_expiry() {
    let old = tempfile::tempdir().unwrap();
    let conn = firefox_cookies_db(&old.path().join("cookies.sqlite"), 12, false);
    fill(&conn, 1, false);
    drop(conn);
    let out = read_cookies(
        &profile("f/old", Family::Gecko, old.path().to_owned()),
        None,
        NOW,
    );
    check(&out.unwrap(), 0);

    let new = tempfile::tempdir().unwrap();
    let conn = firefox_cookies_db(&new.path().join("cookies.sqlite"), 16, true);
    fill(&conn, 1000, true);
    drop(conn);
    let out = read_cookies(
        &profile("f/new", Family::Gecko, new.path().to_owned()),
        None,
        NOW,
    );
    check(&out.unwrap(), 1);
}

#[test]
fn reads_while_a_wal_holds_unflushed_rows() {
    let dir = tempfile::tempdir().unwrap();
    let conn = firefox_cookies_db(&dir.path().join("cookies.sqlite"), 16, true);
    conn.execute_batch("PRAGMA wal_autocheckpoint = 0;")
        .unwrap();
    fill(&conn, 1000, true);
    // `conn` stays open, so the rows live only in cookies.sqlite-wal.
    let out = read_cookies(
        &profile("f/wal", Family::Gecko, dir.path().to_owned()),
        None,
        NOW,
    );
    check(&out.unwrap(), 1);
    drop(conn);
}

#[test]
fn history_visits_newest_first() {
    let dir = tempfile::tempdir().unwrap();
    let conn = firefox_places_db(&dir.path().join("places.sqlite"));
    conn.execute_batch(
        "INSERT INTO moz_places (id, url, title) VALUES
           (1, 'https://mozilla.example/', 'Moz'),
           (2, 'about:config', 'Config'),
           (3, 'http://plain.example/', NULL);",
    )
    .unwrap();
    for (place, micros) in [
        (1, NOW * 1_000_000 + 1_500),
        (3, (NOW - 60) * 1_000_000),
        (1, (NOW - 120) * 1_000_000),
        (2, (NOW + 1) * 1_000_000),
    ] {
        conn.execute(
            "INSERT INTO moz_historyvisits (place_id, visit_date, visit_type) VALUES (?1, ?2, 1)",
            [place, micros],
        )
        .unwrap();
    }
    drop(conn);
    let visits = read_history(&profile("f/p", Family::Gecko, dir.path().to_owned()), 10).unwrap();
    let got: Vec<_> = visits
        .iter()
        .map(|v| (v.url.as_str(), v.title.as_str(), v.visited_at_ms))
        .collect();
    assert_eq!(
        got,
        [
            ("https://mozilla.example/", "Moz", NOW * 1000 + 1),
            ("http://plain.example/", "", (NOW - 60) * 1000),
            ("https://mozilla.example/", "Moz", (NOW - 120) * 1000),
        ]
    );
}
