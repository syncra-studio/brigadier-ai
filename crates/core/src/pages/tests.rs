use std::time::Instant;

use super::*;

/// A pages folder of its own, removed afterwards.
struct Folder(PathBuf);

impl Folder {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("brigadier-pages-{}", uuid::Uuid::now_v7()));
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn pages(&self) -> Pages {
        Pages::unwatched(self.0.join(PAGES_DIR))
    }

    fn watched(&self) -> Pages {
        Pages::open(self.0.join(PAGES_DIR))
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = fs::set_permissions(self.0.join(PAGES_DIR), fs::Permissions::from_mode(0o755));
        }
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn content(title: &str, emoji: Option<&str>, body: &str) -> PageContent {
    PageContent {
        title: title.to_owned(),
        emoji: emoji.map(str::to_owned),
        body: body.to_owned(),
    }
}

fn saved(outcome: PageWriteOutcome) -> String {
    match outcome {
        PageWriteOutcome::Saved { revision } => revision,
        PageWriteOutcome::Conflict { current } => panic!("unexpected conflict: {current:?}"),
    }
}

/// Waits up to `within` for `expected` to be told, passing over others (a file being written
/// can be seen half-written first).
fn told(
    changes: &mut broadcast::Receiver<PageChange>,
    expected: &PageChange,
    within: Duration,
) -> bool {
    let until = Instant::now() + within;
    loop {
        match changes.try_recv() {
            Ok(change) if change == *expected => return true,
            Ok(_) => {}
            Err(broadcast::error::TryRecvError::Empty) if Instant::now() < until => {
                thread::sleep(Duration::from_millis(20));
            }
            Err(_) => return false,
        }
    }
}

#[test]
fn a_page_is_created_read_saved_listed_and_deleted() {
    let folder = Folder::new();
    let pages = folder.pages();
    let page = pages.create().unwrap();
    assert!(is_page_id(&page.id));
    assert_eq!(page.content, PageContent::default());
    assert!(page.created_ms.is_some());
    assert!(pages.path_exists(&page.id));

    let revision = saved(
        pages
            .write(
                &page.id,
                &content("Plans", Some("🗺️"), "# Plans\n\n- one\n"),
                &page.revision,
            )
            .unwrap(),
    );
    assert_ne!(revision, page.revision);
    let read = pages.read(&page.id).unwrap();
    assert_eq!(read.revision, revision);
    assert_eq!(
        read.content,
        content("Plans", Some("🗺️"), "# Plans\n\n- one\n")
    );
    assert_eq!(read.created_ms, page.created_ms, "made once, kept by saves");

    let other = pages.create().unwrap();
    let listed = pages.list().unwrap();
    assert_eq!(listed.len(), 2);
    let plans = listed.iter().find(|summary| summary.id == page.id).unwrap();
    assert_eq!(plans.title, "Plans");
    assert_eq!(plans.emoji.as_deref(), Some("🗺️"));
    assert!(
        listed
            .windows(2)
            .all(|pair| pair[0].updated_ms >= pair[1].updated_ms)
    );

    pages.delete(&page.id).unwrap();
    assert!(matches!(pages.read(&page.id), Err(PageError::NotFound(_))));
    pages
        .delete(&page.id)
        .expect("deleting a page already gone is fine");
    let listed = pages.list().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, other.id);
}

#[test]
fn the_list_puts_the_most_recently_changed_page_first() {
    let folder = Folder::new();
    let pages = folder.pages();
    let older = pages.create().unwrap();
    let newer = pages.create().unwrap();
    let at = |ms: u64| UNIX_EPOCH + Duration::from_millis(ms);
    let touch = |id: &str, ms: u64| {
        fs::File::options()
            .write(true)
            .open(pages.inner.path(id))
            .unwrap()
            .set_modified(at(ms))
            .unwrap();
    };
    touch(&older.id, 2_000_000);
    touch(&newer.id, 1_000_000);
    let ids: Vec<String> = pages.list().unwrap().into_iter().map(|p| p.id).collect();
    assert_eq!(ids, vec![older.id.clone(), newer.id.clone()]);
}

#[test]
fn a_save_based_on_an_old_revision_after_an_outside_edit_is_a_conflict() {
    let folder = Folder::new();
    let pages = folder.pages();
    let page = pages.create().unwrap();
    let mine = saved(
        pages
            .write(&page.id, &content("Mine", None, "mine\n"), &page.revision)
            .unwrap(),
    );
    // Another program rewrites the file.
    let outside = "---\ntitle: \"Theirs\"\n---\n\ntheirs\n";
    fs::write(pages.inner.path(&page.id), outside).unwrap();

    let outcome = pages
        .write(&page.id, &content("Mine again", None, "mine 2\n"), &mine)
        .unwrap();
    let PageWriteOutcome::Conflict { current } = outcome else {
        panic!("expected a conflict, got {outcome:?}");
    };
    assert_eq!(current.content, content("Theirs", None, "theirs\n"));
    assert_eq!(current.revision, revision(outside.as_bytes()));
    assert_eq!(
        fs::read_to_string(pages.inner.path(&page.id)).unwrap(),
        outside,
        "a refused save writes nothing"
    );

    // Based on what is there now, it saves.
    saved(
        pages
            .write(
                &page.id,
                &content("Merged", None, "both\n"),
                &current.revision,
            )
            .unwrap(),
    );
    assert_eq!(pages.read(&page.id).unwrap().content.title, "Merged");
}

#[test]
fn saving_what_is_already_there_keeps_the_revision() {
    let folder = Folder::new();
    let pages = folder.pages();
    let page = pages.create().unwrap();
    let first = saved(
        pages
            .write(&page.id, &content("Same", None, "same\n"), &page.revision)
            .unwrap(),
    );
    let second = saved(
        pages
            .write(&page.id, &content("Same", None, "same\n"), &first)
            .unwrap(),
    );
    assert_eq!(first, second);
}

#[test]
fn a_failed_save_leaves_the_old_file_and_no_temporary_one() {
    let folder = Folder::new();
    let pages = folder.pages();
    let page = pages.create().unwrap();
    let revision = saved(
        pages
            .write(&page.id, &content("Kept", None, "kept\n"), &page.revision)
            .unwrap(),
    );
    let before = fs::read(pages.inner.path(&page.id)).unwrap();

    pages.fail_next_save();
    let failed = pages.write(&page.id, &content("Lost", None, "lost\n"), &revision);
    assert!(
        matches!(failed, Err(PageError::Io { action: "save", .. })),
        "{failed:?}"
    );
    assert_eq!(fs::read(pages.inner.path(&page.id)).unwrap(), before);
    let names: Vec<String> = fs::read_dir(&pages.inner.dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        vec![format!("{}.md", page.id)],
        "the temporary file is removed"
    );

    // The page still saves afterwards, from the same revision.
    saved(
        pages
            .write(&page.id, &content("Saved", None, "saved\n"), &revision)
            .unwrap(),
    );
}

#[cfg(unix)]
#[test]
fn a_save_into_a_read_only_folder_fails_and_leaves_the_old_file() {
    use std::os::unix::fs::PermissionsExt as _;

    let folder = Folder::new();
    let pages = folder.pages();
    let page = pages.create().unwrap();
    let before = fs::read(pages.inner.path(&page.id)).unwrap();
    fs::set_permissions(&pages.inner.dir, fs::Permissions::from_mode(0o555)).unwrap();
    // Root ignores the folder's permissions; nothing to show then.
    if fs::write(pages.inner.dir.join("probe"), "").is_ok() {
        return;
    }
    let failed = pages.write(&page.id, &content("No", None, "no\n"), &page.revision);
    assert!(matches!(failed, Err(PageError::Io { .. })), "{failed:?}");
    assert_eq!(fs::read(pages.inner.path(&page.id)).unwrap(), before);
}

#[test]
fn ids_that_are_not_page_ids_are_refused() {
    let folder = Folder::new();
    let pages = folder.pages();
    // A file next to the pages folder that a path could reach.
    fs::write(folder.0.join("secret.md"), "secret").unwrap();
    let page = pages.create().unwrap();
    let bad = [
        String::new(),
        "..".to_owned(),
        "../secret".to_owned(),
        "..%2Fsecret".to_owned(),
        "/etc/passwd".to_owned(),
        format!("{}/../../secret", page.id),
        format!("{}.md", page.id),
        page.id.to_uppercase(),
        page.id[..31].to_owned(),
        format!("{}0", page.id),
        "g".repeat(32),
        "con".to_owned(),
        format!("..\\{}", &page.id[3..]),
        format!("{}\0", &page.id[1..]),
    ];
    for id in &bad {
        assert!(
            matches!(pages.read(id), Err(PageError::Invalid(_))),
            "read {id:?}"
        );
        assert!(
            matches!(
                pages.write(id, &PageContent::default(), "x"),
                Err(PageError::Invalid(_))
            ),
            "write {id:?}"
        );
        assert!(
            matches!(pages.delete(id), Err(PageError::Invalid(_))),
            "delete {id:?}"
        );
    }
    assert!(folder.0.join("secret.md").exists());
    assert!(pages.read(&page.id).is_ok());
}

#[test]
fn the_list_skips_files_that_are_not_pages() {
    let folder = Folder::new();
    let pages = folder.pages();
    let page = pages.create().unwrap();
    for name in [
        "notes.md",
        ".hidden.md",
        "0123456789abcdef0123456789abcdef.txt",
    ] {
        fs::write(pages.inner.dir.join(name), "x").unwrap();
    }
    fs::write(
        pages.inner.dir.join(format!(".{}.0000.tmp", page.id)),
        "half",
    )
    .unwrap();
    let ids: Vec<String> = pages.list().unwrap().into_iter().map(|p| p.id).collect();
    assert_eq!(ids, vec![page.id]);
}

#[test]
fn the_title_and_emoji_round_trip_through_the_front_matter() {
    let folder = Folder::new();
    let pages = folder.pages();
    let page = pages.create().unwrap();
    let tricky = content(
        "Q3: \"plans\" — it's #1\nsecond line \\ end",
        Some("👩🏽‍💻"),
        "---\nnot front matter\n---\n\n\nbody",
    );
    let revision = saved(pages.write(&page.id, &tricky, &page.revision).unwrap());
    assert_eq!(pages.read(&page.id).unwrap().content, tricky);

    // An emoji taken away goes from the file.
    let plain = content("Plain", None, "");
    saved(pages.write(&page.id, &plain, &revision).unwrap());
    let text = fs::read_to_string(pages.inner.path(&page.id)).unwrap();
    assert!(!text.contains("emoji:"), "{text}");
    assert_eq!(pages.read(&page.id).unwrap().content, plain);
}

#[test]
fn front_matter_written_by_hand_is_read_and_its_other_keys_are_kept() {
    let folder = Folder::new();
    let pages = folder.pages();
    let page = pages.create().unwrap();
    let by_hand = "---\r\ntitle: Written by hand\r\nemoji: '🌱'\r\ntags: [a, b]\r\ncreated: 2026-01-02T03:04:05Z\r\n---\r\nBody\r\n";
    fs::write(pages.inner.path(&page.id), by_hand).unwrap();
    let read = pages.read(&page.id).unwrap();
    assert_eq!(
        read.content,
        content("Written by hand", Some("🌱"), "Body\r\n")
    );
    assert_eq!(read.created_ms, Some(1_767_323_045_000));

    saved(
        pages
            .write(
                &page.id,
                &content("Renamed", Some("🌱"), "Body\n"),
                &read.revision,
            )
            .unwrap(),
    );
    let text = fs::read_to_string(pages.inner.path(&page.id)).unwrap();
    assert!(text.contains("\ntags: [a, b]\n"), "{text}");
    assert!(text.contains("\ncreated: 2026-01-02T03:04:05Z\n"), "{text}");
    let read = pages.read(&page.id).unwrap();
    assert_eq!(read.content, content("Renamed", Some("🌱"), "Body\n"));
    assert_eq!(read.created_ms, Some(1_767_323_045_000));
}

#[test]
fn a_file_without_front_matter_is_all_body() {
    let folder = Folder::new();
    let pages = folder.pages();
    let page = pages.create().unwrap();
    for text in ["# Just markdown\n", "---\nnever closed\n", ""] {
        fs::write(pages.inner.path(&page.id), text).unwrap();
        let read = pages.read(&page.id).unwrap();
        assert_eq!(read.content, content("", None, text));
        assert_eq!(read.created_ms, None);
    }
}

#[test]
fn saves_and_deletes_tell_listeners_once() {
    let folder = Folder::new();
    let pages = folder.pages();
    let mut changes = pages.subscribe();
    let page = pages.create().unwrap();
    assert_eq!(
        changes.try_recv().unwrap(),
        PageChange {
            id: page.id.clone(),
            revision: Some(page.revision.clone()),
        }
    );
    let revision = saved(
        pages
            .write(&page.id, &content("Told", None, ""), &page.revision)
            .unwrap(),
    );
    assert_eq!(changes.try_recv().unwrap().revision, Some(revision.clone()));
    // Reading and saving the same again change nothing on disk.
    pages.read(&page.id).unwrap();
    saved(
        pages
            .write(&page.id, &content("Told", None, ""), &revision)
            .unwrap(),
    );
    pages.rescan();
    assert!(changes.try_recv().is_err());

    pages.delete(&page.id).unwrap();
    assert_eq!(
        changes.try_recv().unwrap(),
        PageChange {
            id: page.id.clone(),
            revision: None,
        }
    );
}

#[test]
fn an_outside_edit_is_told_when_the_page_is_next_read() {
    let folder = Folder::new();
    let pages = folder.pages();
    let page = pages.create().unwrap();
    let mut changes = pages.subscribe();
    fs::write(pages.inner.path(&page.id), "edited elsewhere\n").unwrap();
    let read = pages.read(&page.id).unwrap();
    assert_eq!(
        changes.try_recv().unwrap(),
        PageChange {
            id: page.id,
            revision: Some(read.revision),
        }
    );
}

#[test]
fn the_watcher_tells_outside_edits_new_files_and_deletions() {
    let folder = Folder::new();
    let pages = folder.watched();
    let page = pages.create().unwrap();
    let mut changes = pages.subscribe();
    // Generous: file events can take a while on a busy machine.
    let within = Duration::from_secs(10);

    let edited = "---\ntitle: \"Elsewhere\"\n---\n\nedited\n";
    fs::write(pages.inner.path(&page.id), edited).unwrap();
    let expected = PageChange {
        id: page.id.clone(),
        revision: Some(revision(edited.as_bytes())),
    };
    assert!(told(&mut changes, &expected, within), "the outside edit");

    let new_id = uuid::Uuid::now_v7().simple().to_string();
    fs::write(pages.inner.path(&new_id), "dropped in\n").unwrap();
    let expected = PageChange {
        id: new_id,
        revision: Some(revision(b"dropped in\n")),
    };
    assert!(told(&mut changes, &expected, within), "the new file");

    fs::remove_file(pages.inner.path(&page.id)).unwrap();
    let expected = PageChange {
        id: page.id,
        revision: None,
    };
    assert!(told(&mut changes, &expected, within), "the deletion");
}

impl Pages {
    fn path_exists(&self, id: &str) -> bool {
        self.inner.path(id).is_file()
    }
}
