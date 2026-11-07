//! Reading bookmarks from fixtures of each engine's files.

mod common;

use std::collections::BTreeMap;
use std::path::Path;

use brigadier_browser_data::{Bookmark, BookmarkRoot, Error, Family, read_bookmarks};
use common::*;
use plist::{Dictionary, Value};
use rusqlite::params;

/// Each file under `dir` and its bytes, so a test can show a read changed nothing.
fn snapshot(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::new();
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        files.insert(
            entry.file_name().to_string_lossy().into_owned(),
            std::fs::read(entry.path()).unwrap(),
        );
    }
    files
}

fn bookmark(
    url: &str,
    title: &str,
    root: BookmarkRoot,
    folders: &[&str],
    added_at_ms: Option<i64>,
) -> Bookmark {
    Bookmark {
        url: url.to_owned(),
        title: title.to_owned(),
        root,
        folders: folders.iter().map(|f| (*f).to_owned()).collect(),
        added_at_ms,
    }
}

#[test]
fn chromium_bookmarks_keep_their_folders_order_and_dates() {
    let dir = tempfile::tempdir().unwrap();
    let added = chromium_time(NOW);
    std::fs::write(
        dir.path().join("Bookmarks"),
        format!(
            r#"{{"checksum":"x","version":1,"roots":{{
              "bookmark_bar":{{"type":"folder","name":"Bookmarks bar","children":[
                {{"type":"url","name":"One","url":"https://one.example/","date_added":"{added}"}},
                {{"type":"folder","name":"Work","children":[
                  {{"type":"folder","name":"Deep","children":[
                    {{"type":"url","name":"Two","url":"http://two.example/","date_added":"0"}}
                  ]}}
                ]}},
                {{"type":"url","name":"Script","url":"javascript:alert(1)"}},
                {{"type":"url","name":"Internal","url":"chrome://settings"}}
              ]}},
              "other":{{"type":"folder","name":"Other","children":[
                {{"type":"url","name":"Three","url":"https://three.example/"}}
              ]}},
              "synced":{{"type":"folder","name":"Mobile","children":[
                {{"type":"url","name":"Four","url":"https://four.example/"}}
              ]}}
            }}}}"#
        ),
    )
    .unwrap();
    std::fs::write(
        dir.path().join("AccountBookmarks"),
        r#"{"roots":{"bookmark_bar":{"type":"folder","children":[
            {"type":"url","name":"Five","url":"https://five.example/"}]}}}"#,
    )
    .unwrap();
    let before = snapshot(dir.path());

    let read = read_bookmarks(&profile(
        "chrome/Default",
        Family::Chromium,
        dir.path().to_owned(),
    ))
    .unwrap();

    assert_eq!(
        read.bookmarks,
        [
            bookmark(
                "https://one.example/",
                "One",
                BookmarkRoot::Bar,
                &[],
                Some(NOW * 1000)
            ),
            bookmark(
                "http://two.example/",
                "Two",
                BookmarkRoot::Bar,
                &["Work", "Deep"],
                None
            ),
            bookmark(
                "https://three.example/",
                "Three",
                BookmarkRoot::Other,
                &[],
                None
            ),
            bookmark(
                "https://four.example/",
                "Four",
                BookmarkRoot::Mobile,
                &[],
                None
            ),
            bookmark(
                "https://five.example/",
                "Five",
                BookmarkRoot::Bar,
                &[],
                None
            ),
        ]
    );
    assert_eq!(read.skipped, 2);
    assert_eq!(snapshot(dir.path()), before);
}

#[test]
fn chromium_bookmarks_report_what_is_wrong() {
    let dir = tempfile::tempdir().unwrap();
    let chrome = profile("chrome/Default", Family::Chromium, dir.path().to_owned());
    assert!(matches!(read_bookmarks(&chrome), Err(Error::NotFound(_))));
    std::fs::write(dir.path().join("Bookmarks"), "not json").unwrap();
    assert!(matches!(read_bookmarks(&chrome), Err(Error::Format(_))));
    std::fs::write(dir.path().join("Bookmarks"), "{}").unwrap();
    assert!(matches!(read_bookmarks(&chrome), Err(Error::Format(_))));
}

#[test]
fn firefox_bookmarks_come_from_the_visible_roots_only() {
    let dir = tempfile::tempdir().unwrap();
    let places = dir.path().join("places.sqlite");
    let conn = firefox_places_db(&places);
    firefox_bookmark_roots(&conn);
    let added_micros = NOW * 1_000_000;
    let add_place = |id: i64, url: &str, title: Option<&str>| {
        conn.execute(
            "INSERT INTO moz_places (id, url, title) VALUES (?1, ?2, ?3)",
            params![id, url, title],
        )
        .unwrap();
    };
    add_place(1, "https://one.example/", Some("Page title"));
    add_place(2, "https://two.example/", None);
    add_place(3, "https://three.example/", None);
    add_place(4, "javascript:alert(1)", None);
    add_place(5, "https://tagged.example/", None);
    conn.execute_batch(&format!(
        "INSERT INTO moz_bookmarks (id, type, fk, parent, position, title, dateAdded, guid) VALUES
           (10, 1, 1, 3, 1, NULL, {added_micros}, 'a'),        -- title from moz_places
           (11, 2, NULL, 3, 0, 'Work', {added_micros}, 'b'),   -- a folder first
           (12, 1, 2, 11, 0, 'Two', {added_micros}, 'c'),
           (13, 3, NULL, 3, 2, NULL, {added_micros}, 'd'),     -- a separator
           (14, 1, 3, 2, 0, 'Three', 0, 'e'),                  -- the menu, no date
           (15, 1, 4, 5, 0, 'Script', {added_micros}, 'f'),    -- not a web page
           (16, 1, 3, 6, 0, 'Phone', {added_micros}, 'g'),     -- synced from a phone
           (17, 2, NULL, 4, 0, 'a-tag', {added_micros}, 'h'),  -- a tag, not a folder
           (18, 1, 5, 17, 0, NULL, {added_micros}, 'i'),       -- a tag's entry
           (19, 2, NULL, 3, 3, 'Loop', {added_micros}, 'j');   -- damaged: see below
         UPDATE moz_bookmarks SET parent = 19 WHERE id = 3;     -- the toolbar is inside it",
    ))
    .unwrap();
    drop(conn);
    let before = snapshot(dir.path());

    let read = read_bookmarks(&profile("firefox/x", Family::Gecko, dir.path().to_owned())).unwrap();

    let at = Some(NOW * 1000);
    assert_eq!(
        read.bookmarks,
        [
            bookmark(
                "https://two.example/",
                "Two",
                BookmarkRoot::Bar,
                &["Work"],
                at
            ),
            bookmark(
                "https://one.example/",
                "Page title",
                BookmarkRoot::Bar,
                &[],
                at
            ),
            bookmark(
                "https://three.example/",
                "Three",
                BookmarkRoot::Menu,
                &[],
                None
            ),
            bookmark(
                "https://three.example/",
                "Phone",
                BookmarkRoot::Mobile,
                &[],
                at
            ),
        ]
    );
    assert_eq!(read.skipped, 1);
    assert_eq!(snapshot(dir.path()), before);
}

fn dict(entries: Vec<(&str, Value)>) -> Value {
    let mut dict = Dictionary::new();
    for (key, value) in entries {
        dict.insert(key.to_owned(), value);
    }
    Value::Dictionary(dict)
}

fn safari_list(title: &str, children: Vec<Value>) -> Value {
    dict(vec![
        ("WebBookmarkType", Value::from("WebBookmarkTypeList")),
        ("Title", Value::from(title)),
        ("Children", Value::Array(children)),
    ])
}

fn safari_leaf(url: &str, title: &str) -> Value {
    dict(vec![
        ("WebBookmarkType", Value::from("WebBookmarkTypeLeaf")),
        ("URLString", Value::from(url)),
        ("URIDictionary", dict(vec![("title", Value::from(title))])),
    ])
}

#[test]
fn safari_bookmarks_from_a_binary_plist() {
    let dir = tempfile::tempdir().unwrap();
    let history_proxy = dict(vec![
        ("WebBookmarkType", Value::from("WebBookmarkTypeProxy")),
        ("Title", Value::from("History")),
    ]);
    let tree = safari_list(
        "",
        vec![
            history_proxy,
            safari_list(
                "BookmarksBar",
                vec![
                    safari_leaf("https://one.example/", "One"),
                    safari_list("Work", vec![safari_leaf("https://two.example/", "Two")]),
                    safari_leaf("javascript:alert(1)", "Script"),
                ],
            ),
            safari_list(
                "BookmarksMenu",
                vec![safari_leaf("https://three.example/", "Three")],
            ),
            safari_list(
                "com.apple.ReadingList",
                vec![safari_leaf("https://four.example/", "Four")],
            ),
            safari_list("Mine", vec![safari_leaf("https://five.example/", "Five")]),
            safari_leaf("https://six.example/", "Six"),
        ],
    );
    let path = dir.path().join("Bookmarks.plist");
    tree.to_file_binary(&path).unwrap();
    let before = snapshot(dir.path());

    let read = read_bookmarks(&profile(
        "safari/default",
        Family::Safari,
        dir.path().to_owned(),
    ))
    .unwrap();

    assert_eq!(
        read.bookmarks,
        [
            bookmark("https://one.example/", "One", BookmarkRoot::Bar, &[], None),
            bookmark(
                "https://two.example/",
                "Two",
                BookmarkRoot::Bar,
                &["Work"],
                None
            ),
            bookmark(
                "https://three.example/",
                "Three",
                BookmarkRoot::Menu,
                &[],
                None
            ),
            bookmark(
                "https://four.example/",
                "Four",
                BookmarkRoot::Other,
                &["Reading List"],
                None
            ),
            bookmark(
                "https://five.example/",
                "Five",
                BookmarkRoot::Other,
                &["Mine"],
                None
            ),
            bookmark(
                "https://six.example/",
                "Six",
                BookmarkRoot::Other,
                &[],
                None
            ),
        ]
    );
    assert_eq!(read.skipped, 1);
    assert_eq!(snapshot(dir.path()), before);
}

#[test]
fn safari_bookmarks_report_what_is_wrong() {
    let dir = tempfile::tempdir().unwrap();
    let safari = profile("safari/default", Family::Safari, dir.path().to_owned());
    assert!(matches!(read_bookmarks(&safari), Err(Error::NotFound(_))));
    std::fs::write(dir.path().join("Bookmarks.plist"), "nope").unwrap();
    assert!(matches!(read_bookmarks(&safari), Err(Error::Format(_))));
}
