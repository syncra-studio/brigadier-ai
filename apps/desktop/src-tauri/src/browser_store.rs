//! What the built-in browser remembers beside its pages' own data (which WebKit or WebView2
//! keeps in the browser's saved store): the pages it showed, its downloads, which sites may
//! use the camera or microphone, and what the last import from each other browser brought. One SQLite file, `browser.sqlite`, in the browser's folder of
//! Brigadier's data folder; nothing here leaves the machine.

use std::path::Path;
use std::sync::Mutex;

use brigadier_ipc::app::{
    BrowserDownload, BrowserSite, BrowserVisit, DownloadState, HistoryFilter, ImportRecord,
    SitePermission,
};
use rusqlite::{Connection, params};

/// The schema, by `PRAGMA user_version`.
const SCHEMA: &[&str] = &[
    r"
    CREATE TABLE visits (
        id INTEGER PRIMARY KEY,
        url TEXT NOT NULL,
        title TEXT NOT NULL DEFAULT '',
        at_ms INTEGER NOT NULL,
        agent INTEGER NOT NULL DEFAULT 0
    );
    CREATE INDEX visits_at ON visits (at_ms);
    CREATE TABLE downloads (
        id INTEGER PRIMARY KEY,
        url TEXT NOT NULL,
        path TEXT NOT NULL,
        at_ms INTEGER NOT NULL,
        state TEXT NOT NULL
    );
    CREATE TABLE sites (
        origin TEXT PRIMARY KEY,
        camera TEXT NOT NULL DEFAULT 'ask',
        microphone TEXT NOT NULL DEFAULT 'ask',
        asked_ms INTEGER NOT NULL
    );
",
    r"
    CREATE TABLE imports (
        browser TEXT PRIMARY KEY,
        record TEXT,
        keychain_denied INTEGER NOT NULL DEFAULT 0
    );
",
];

/// The most pages a history list returns at once.
const PAGE_LIMIT: u32 = 500;

pub struct Store {
    db: Connection,
}

/// The camera or the microphone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Device {
    Camera,
    Microphone,
}

impl Device {
    fn column(self) -> &'static str {
        match self {
            Device::Camera => "camera",
            Device::Microphone => "microphone",
        }
    }
}

fn permission_text(permission: SitePermission) -> &'static str {
    match permission {
        SitePermission::Ask => "ask",
        SitePermission::Allow => "allow",
        SitePermission::Block => "block",
    }
}

fn permission(text: &str) -> SitePermission {
    match text {
        "allow" => SitePermission::Allow,
        "block" => SitePermission::Block,
        _ => SitePermission::Ask,
    }
}

#[cfg(any(target_os = "macos", target_os = "windows", test))]
fn state_text(state: DownloadState) -> &'static str {
    match state {
        DownloadState::Running => "running",
        DownloadState::Done => "done",
        DownloadState::Failed => "failed",
    }
}

fn state(text: &str) -> DownloadState {
    match text {
        "done" => DownloadState::Done,
        "running" => DownloadState::Running,
        _ => DownloadState::Failed,
    }
}

/// `query` as a LIKE pattern matching it anywhere, its own `%`, `_` and `\` taken literally.
fn like(query: &str) -> String {
    let mut pattern = String::from("%");
    for c in query.trim().chars() {
        if matches!(c, '%' | '_' | '\\') {
            pattern.push('\\');
        }
        pattern.push(c);
    }
    pattern.push('%');
    pattern
}

impl Store {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        Self::with(Connection::open(path)?)
    }

    #[cfg(test)]
    pub fn in_memory() -> Self {
        Self::with(Connection::open_in_memory().expect("in-memory db")).expect("schema")
    }

    fn with(db: Connection) -> rusqlite::Result<Self> {
        db.pragma_update(None, "journal_mode", "WAL")?;
        let version: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
        for (at, step) in (0_i64..).zip(SCHEMA).skip(version as usize) {
            db.execute_batch(step)?;
            db.pragma_update(None, "user_version", at + 1)?;
        }
        // The store is opened once per run: a download still running is one the last run left
        // unfinished, and nothing resumes it.
        db.execute(
            "UPDATE downloads SET state = 'failed' WHERE state = 'running'",
            [],
        )?;
        Ok(Self { db })
    }

    /// The page at `url` finished loading, with the title it showed while loading, if any. A
    /// reload of the page just shown moves that visit's time (and takes a new title) instead of
    /// adding one. Returns the visit, whose title may arrive later.
    #[cfg(any(target_os = "macos", target_os = "windows", test))]
    pub fn visit(
        &self,
        last: Option<i64>,
        url: &str,
        title: &str,
        agent: bool,
        at_ms: i64,
    ) -> rusqlite::Result<i64> {
        use rusqlite::OptionalExtension;

        if let Some(id) = last {
            let same = self
                .db
                .query_row("SELECT url FROM visits WHERE id = ?1", [id], |row| {
                    row.get::<_, String>(0)
                })
                .optional()?
                .is_some_and(|shown| shown == url);
            if same {
                self.db.execute(
                    "UPDATE visits SET at_ms = ?2, title = CASE WHEN ?3 = '' THEN title ELSE ?3 END
                     WHERE id = ?1",
                    params![id, at_ms, title],
                )?;
                return Ok(id);
            }
        }
        self.db.execute(
            "INSERT INTO visits (url, title, at_ms, agent) VALUES (?1, ?2, ?3, ?4)",
            params![url, title, at_ms, agent],
        )?;
        Ok(self.db.last_insert_rowid())
    }

    #[cfg(any(target_os = "macos", target_os = "windows", test))]
    pub fn titled(&self, id: i64, title: &str) -> rusqlite::Result<()> {
        self.db.execute(
            "UPDATE visits SET title = ?2 WHERE id = ?1",
            params![id, title],
        )?;
        Ok(())
    }

    /// Visits newest first, after the visit `before` (its time and id) when given, whose
    /// address or title holds `query`. The id breaks ties, so a page boundary between visits at
    /// the same millisecond skips none of them.
    pub fn history(
        &self,
        query: &str,
        filter: HistoryFilter,
        before: Option<(i64, i64)>,
        limit: u32,
    ) -> rusqlite::Result<Vec<BrowserVisit>> {
        let agent = match filter {
            HistoryFilter::All => None,
            HistoryFilter::Agent => Some(true),
            HistoryFilter::Other => Some(false),
        };
        let mut statement = self.db.prepare_cached(
            "SELECT id, url, title, at_ms, agent FROM visits
             WHERE (url LIKE ?1 ESCAPE '\\' OR title LIKE ?1 ESCAPE '\\')
               AND (?2 IS NULL OR agent = ?2)
               AND (?3 IS NULL OR at_ms < ?3 OR (at_ms = ?3 AND id < ?5))
             ORDER BY at_ms DESC, id DESC LIMIT ?4",
        )?;
        statement
            .query_map(
                params![
                    like(query),
                    agent,
                    before.map(|(at_ms, _)| at_ms),
                    limit.min(PAGE_LIMIT),
                    before.map(|(_, id)| id),
                ],
                |row| {
                    Ok(BrowserVisit {
                        id: row.get(0)?,
                        url: row.get(1)?,
                        title: row.get(2)?,
                        at_ms: row.get(3)?,
                        agent: row.get(4)?,
                    })
                },
            )?
            .collect()
    }

    pub fn remove_visits(&self, ids: &[i64]) -> rusqlite::Result<()> {
        let mut statement = self.db.prepare_cached("DELETE FROM visits WHERE id = ?1")?;
        for id in ids {
            statement.execute([id])?;
        }
        Ok(())
    }

    /// Adds pages another browser showed, as the user's own; ones already here at the same time
    /// are skipped. Returns how many were added.
    pub fn import_visits<'a>(
        &mut self,
        visits: impl IntoIterator<Item = (&'a str, &'a str, i64)>,
    ) -> rusqlite::Result<usize> {
        let transaction = self.db.transaction()?;
        let mut added = 0;
        {
            let mut known =
                transaction.prepare_cached("SELECT 1 FROM visits WHERE url = ?1 AND at_ms = ?2")?;
            let mut insert = transaction.prepare_cached(
                "INSERT INTO visits (url, title, at_ms, agent) VALUES (?1, ?2, ?3, 0)",
            )?;
            for (url, title, at_ms) in visits {
                if !known.exists(params![url, at_ms])? {
                    insert.execute(params![url, title, at_ms])?;
                    added += 1;
                }
            }
        }
        transaction.commit()?;
        Ok(added)
    }

    pub fn clear_history(&self, since_ms: Option<i64>) -> rusqlite::Result<()> {
        self.db.execute(
            "DELETE FROM visits WHERE ?1 IS NULL OR at_ms >= ?1",
            [since_ms],
        )?;
        Ok(())
    }

    #[cfg(any(target_os = "macos", target_os = "windows", test))]
    pub fn download_started(&self, url: &str, path: &str, at_ms: i64) -> rusqlite::Result<i64> {
        self.db.execute(
            "INSERT INTO downloads (url, path, at_ms, state) VALUES (?1, ?2, ?3, ?4)",
            params![url, path, at_ms, state_text(DownloadState::Running)],
        )?;
        Ok(self.db.last_insert_rowid())
    }

    #[cfg(any(target_os = "macos", target_os = "windows", test))]
    pub fn download_finished(&self, id: i64, done: bool) -> rusqlite::Result<()> {
        let state = if done {
            DownloadState::Done
        } else {
            DownloadState::Failed
        };
        self.db.execute(
            "UPDATE downloads SET state = ?2 WHERE id = ?1",
            params![id, state_text(state)],
        )?;
        Ok(())
    }

    /// Downloads newest first, whose address or file holds `query`.
    pub fn downloads(&self, query: &str) -> rusqlite::Result<Vec<BrowserDownload>> {
        let mut statement = self.db.prepare_cached(
            "SELECT id, url, path, at_ms, state FROM downloads
             WHERE url LIKE ?1 ESCAPE '\\' OR path LIKE ?1 ESCAPE '\\'
             ORDER BY at_ms DESC, id DESC LIMIT ?2",
        )?;
        statement
            .query_map(params![like(query), PAGE_LIMIT], |row| {
                Ok(BrowserDownload {
                    id: row.get(0)?,
                    url: row.get(1)?,
                    path: row.get(2)?,
                    at_ms: row.get(3)?,
                    state: state(&row.get::<_, String>(4)?),
                })
            })?
            .collect()
    }

    /// Forgets downloads (the files stay): `ids`, or with none every finished one since
    /// `since_ms`.
    pub fn remove_downloads(
        &self,
        ids: Option<&[i64]>,
        since_ms: Option<i64>,
    ) -> rusqlite::Result<()> {
        match ids {
            Some(ids) => {
                let mut statement = self
                    .db
                    .prepare_cached("DELETE FROM downloads WHERE id = ?1")?;
                for id in ids {
                    statement.execute([id])?;
                }
            }
            None => {
                self.db.execute(
                    "DELETE FROM downloads WHERE state != 'running' AND (?1 IS NULL OR at_ms >= ?1)",
                    [since_ms],
                )?;
            }
        }
        Ok(())
    }

    /// The site `origin` asked for `device`: it is listed from now on. Returns the choice.
    // Only macOS's WebKit asks for camera and microphone through Brigadier.
    #[cfg(any(target_os = "macos", test))]
    pub fn site_asked(
        &self,
        origin: &str,
        device: Device,
        at_ms: i64,
    ) -> rusqlite::Result<SitePermission> {
        self.db.execute(
            "INSERT INTO sites (origin, asked_ms) VALUES (?1, ?2)
             ON CONFLICT (origin) DO UPDATE SET asked_ms = excluded.asked_ms",
            params![origin, at_ms],
        )?;
        let text: String = self.db.query_row(
            &format!("SELECT {} FROM sites WHERE origin = ?1", device.column()),
            [origin],
            |row| row.get(0),
        )?;
        Ok(permission(&text))
    }

    pub fn sites(&self) -> rusqlite::Result<Vec<BrowserSite>> {
        let mut statement = self.db.prepare_cached(
            "SELECT origin, camera, microphone, asked_ms FROM sites ORDER BY origin",
        )?;
        statement
            .query_map([], |row| {
                Ok(BrowserSite {
                    origin: row.get(0)?,
                    camera: permission(&row.get::<_, String>(1)?),
                    microphone: permission(&row.get::<_, String>(2)?),
                    asked_ms: row.get(3)?,
                })
            })?
            .collect()
    }

    pub fn set_site(
        &self,
        origin: &str,
        device: Device,
        choice: SitePermission,
    ) -> rusqlite::Result<()> {
        self.db.execute(
            &format!(
                "UPDATE sites SET {} = ?2 WHERE origin = ?1",
                device.column()
            ),
            params![origin, permission_text(choice)],
        )?;
        Ok(())
    }

    /// Forgets the sites that asked since `since_ms` (all with none), and their choices.
    /// What the last import from each browser brought, by browser key, and whether the last
    /// try was refused the browser's Keychain key.
    pub fn imports(
        &self,
    ) -> rusqlite::Result<std::collections::HashMap<String, (Option<ImportRecord>, bool)>> {
        let mut stmt = self
            .db
            .prepare("SELECT browser, record, keychain_denied FROM imports")?;
        let rows = stmt.query_map([], |row| {
            let record: Option<String> = row.get(1)?;
            Ok((
                row.get::<_, String>(0)?,
                (
                    record.and_then(|json| serde_json::from_str(&json).ok()),
                    row.get::<_, i64>(2)? != 0,
                ),
            ))
        })?;
        rows.collect()
    }

    /// Keeps what an import from `browser` brought, in place of the last; it got the Keychain
    /// key if it needed one, so a refusal before is forgotten.
    pub fn save_import(&self, browser: &str, record: &ImportRecord) -> rusqlite::Result<()> {
        let json = serde_json::to_string(record)
            .map_err(|err| rusqlite::Error::ToSqlConversionFailure(Box::new(err)))?;
        self.db.execute(
            "INSERT INTO imports (browser, record, keychain_denied) VALUES (?1, ?2, 0)
             ON CONFLICT (browser) DO UPDATE SET record = ?2, keychain_denied = 0",
            params![browser, json],
        )?;
        Ok(())
    }

    /// An import from `browser` was refused its Keychain key; its last record stays.
    pub fn import_denied(&self, browser: &str) -> rusqlite::Result<()> {
        self.db.execute(
            "INSERT INTO imports (browser, keychain_denied) VALUES (?1, 1)
             ON CONFLICT (browser) DO UPDATE SET keychain_denied = 1",
            params![browser],
        )?;
        Ok(())
    }

    pub fn clear_sites(&self, since_ms: Option<i64>) -> rusqlite::Result<()> {
        self.db.execute(
            "DELETE FROM sites WHERE ?1 IS NULL OR asked_ms >= ?1",
            [since_ms],
        )?;
        Ok(())
    }
}

/// The answer to a page asking for several devices at once: blocked if any is, allowed only if
/// all are, else the system asks.
#[cfg(any(target_os = "macos", test))]
pub fn combined(choices: &[SitePermission]) -> SitePermission {
    if choices.contains(&SitePermission::Block) || choices.is_empty() {
        SitePermission::Block
    } else if choices
        .iter()
        .all(|choice| *choice == SitePermission::Allow)
    {
        SitePermission::Allow
    } else {
        SitePermission::Ask
    }
}

/// The browser's store, opened on first use.
static STORE: Mutex<Option<Store>> = Mutex::new(None);

/// Runs `action` on the browser's store; `None` when it can't be opened (no data folder yet,
/// or the file is unusable: then the browser just remembers nothing).
pub fn with<T>(action: impl FnOnce(&mut Store) -> rusqlite::Result<T>) -> Option<T> {
    let mut store = STORE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if store.is_none() {
        let dir = crate::browser::data_dir()?;
        match Store::open(&dir.join("browser.sqlite")) {
            Ok(opened) => *store = Some(opened),
            Err(err) => {
                tracing::warn!(error = %err, "the browser's history can't be opened");
                return None;
            }
        }
    }
    match action(store.as_mut()?) {
        Ok(value) => Some(value),
        Err(err) => {
            tracing::warn!(error = %err, "the browser's history store failed");
            None
        }
    }
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_import_per_browser_is_kept_and_a_keychain_refusal_until_the_next() {
        let store = Store::in_memory();
        assert!(store.imports().unwrap().is_empty());
        store.import_denied("dia").unwrap();
        assert_eq!(store.imports().unwrap()["dia"], (None, true));
        let record = ImportRecord {
            at_ms: 1_000,
            profiles: vec!["Work".into(), "HostPapa".into()],
            imported: 2_790,
            skipped: 51,
            skipped_domains: vec!["a.com".into(), "b.com".into()],
            skipped_domain_count: 49,
            history: 120,
        };
        store.save_import("dia", &record).unwrap();
        assert_eq!(
            store.imports().unwrap()["dia"],
            (Some(record.clone()), false)
        );
        // A refusal later keeps the record; a second import replaces it.
        store.import_denied("dia").unwrap();
        assert_eq!(
            store.imports().unwrap()["dia"],
            (Some(record.clone()), true)
        );
        let again = ImportRecord {
            at_ms: 2_000,
            imported: 3,
            ..record
        };
        store.save_import("dia", &again).unwrap();
        store
            .save_import("safari", &ImportRecord::default())
            .unwrap();
        let all = store.imports().unwrap();
        assert_eq!(all["dia"], (Some(again), false));
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn a_store_from_before_imports_gains_the_table() {
        let dir = std::env::temp_dir().join(format!(
            "brigadier-browser-store-imports-{}",
            std::process::id()
        ));
        let path = dir.join("browser.sqlite");
        std::fs::create_dir_all(&dir).unwrap();
        {
            let db = Connection::open(&path).unwrap();
            db.execute_batch(SCHEMA[0]).unwrap();
            db.pragma_update(None, "user_version", 1).unwrap();
            db.execute(
                "INSERT INTO visits (url, at_ms) VALUES ('https://kept.test/', 5)",
                [],
            )
            .unwrap();
        }
        let store = Store::open(&path).unwrap();
        store.save_import("arc", &ImportRecord::default()).unwrap();
        let kept = store
            .history("", HistoryFilter::All, None, 10)
            .unwrap()
            .len();
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(kept, 1);
    }

    #[test]
    fn a_reload_moves_the_visit_and_a_new_page_adds_one() {
        let store = Store::in_memory();
        let first = store.visit(None, "https://a.test/", "", false, 10).unwrap();
        store.titled(first, "A").unwrap();
        assert_eq!(
            store
                .visit(Some(first), "https://a.test/", "", false, 20)
                .unwrap(),
            first
        );
        let second = store
            .visit(Some(first), "https://b.test/", "", true, 30)
            .unwrap();
        assert_ne!(second, first);
        let all = store.history("", HistoryFilter::All, None, 50).unwrap();
        assert_eq!(
            all.iter()
                .map(|v| (v.url.as_str(), v.title.as_str(), v.at_ms, v.agent))
                .collect::<Vec<_>>(),
            [
                ("https://b.test/", "", 30, true),
                ("https://a.test/", "A", 20, false)
            ]
        );
    }

    #[test]
    fn history_filters_by_who_opened_it_searches_and_pages() {
        let store = Store::in_memory();
        store
            .visit(None, "https://agent.test/x", "Preview", true, 1)
            .unwrap();
        store
            .visit(None, "https://mine.test/100%", "News", false, 2)
            .unwrap();
        store
            .visit(None, "https://mine.test/other", "Docs", false, 3)
            .unwrap();
        let urls = |filter, query: &str, before| {
            store
                .history(query, filter, before, 50)
                .unwrap()
                .into_iter()
                .map(|v| v.url)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            urls(HistoryFilter::Agent, "", None),
            ["https://agent.test/x"]
        );
        assert_eq!(urls(HistoryFilter::Other, "", None).len(), 2);
        assert_eq!(
            urls(HistoryFilter::All, "preview", None),
            ["https://agent.test/x"]
        );
        // `%` is matched as itself, not as a wildcard.
        assert_eq!(
            urls(HistoryFilter::All, "100%", None),
            ["https://mine.test/100%"]
        );
        assert_eq!(urls(HistoryFilter::All, "0%o", None), Vec::<String>::new());
        assert_eq!(urls(HistoryFilter::All, "", Some((3, 3))).len(), 2);
    }

    #[test]
    fn a_page_boundary_between_visits_at_the_same_time_skips_none() {
        let store = Store::in_memory();
        for n in 0..5 {
            store
                .visit(None, &format!("https://site.test/{n}"), "", false, 7)
                .unwrap();
        }
        let mut seen = Vec::new();
        let mut before = None;
        loop {
            let page = store.history("", HistoryFilter::All, before, 2).unwrap();
            let Some(last) = page.last() else { break };
            before = Some((last.at_ms, last.id));
            seen.extend(page.into_iter().map(|visit| visit.id));
        }
        assert_eq!(seen, [5, 4, 3, 2, 1]);
    }

    #[test]
    fn clearing_and_removing_history() {
        let mut store = Store::in_memory();
        let old = store
            .visit(None, "https://old.test/", "", false, 100)
            .unwrap();
        store
            .visit(None, "https://new.test/", "", false, 200)
            .unwrap();
        store.clear_history(Some(150)).unwrap();
        assert_eq!(
            store
                .history("", HistoryFilter::All, None, 50)
                .unwrap()
                .len(),
            1
        );
        store.remove_visits(&[old]).unwrap();
        assert!(
            store
                .history("", HistoryFilter::All, None, 50)
                .unwrap()
                .is_empty()
        );
        let added = store
            .import_visits([
                ("https://x.test/", "X", 5),
                ("https://x.test/", "X", 5),
                ("https://y.test/", "", 6),
            ])
            .unwrap();
        assert_eq!(added, 2);
        assert_eq!(
            store.import_visits([("https://x.test/", "X", 5)]).unwrap(),
            0
        );
        store.clear_history(None).unwrap();
        assert!(
            store
                .history("", HistoryFilter::All, None, 50)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn downloads_are_listed_finished_and_forgotten() {
        let store = Store::in_memory();
        let done = store
            .download_started("https://a.test/f.zip", "/d/f.zip", 1)
            .unwrap();
        let running = store
            .download_started("https://a.test/g.zip", "/d/g.zip", 2)
            .unwrap();
        store.download_finished(done, true).unwrap();
        let listed = store.downloads("").unwrap();
        assert_eq!(listed[0].id, running);
        assert_eq!(listed[1].state, DownloadState::Done);
        assert_eq!(store.downloads("f.zip").unwrap().len(), 1);
        // Clearing keeps one still running.
        store.remove_downloads(None, None).unwrap();
        assert_eq!(store.downloads("").unwrap().len(), 1);
        store.remove_downloads(Some(&[running]), None).unwrap();
        assert!(store.downloads("").unwrap().is_empty());
    }

    #[test]
    fn a_download_the_last_run_left_running_reads_as_failed() {
        let dir =
            std::env::temp_dir().join(format!("brigadier-browser-store-{}", std::process::id()));
        let path = dir.join("browser.sqlite");
        let store = Store::open(&path).unwrap();
        let left = store
            .download_started("https://a.test/f.zip", "/d/f.zip", 1)
            .unwrap();
        drop(store);
        let store = Store::open(&path).unwrap();
        let listed = store.downloads("").unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            (listed[0].id, listed[0].state),
            (left, DownloadState::Failed)
        );
    }

    #[test]
    fn asking_for_both_devices_needs_both_allowed() {
        use SitePermission::{Allow, Ask, Block};
        assert_eq!(combined(&[Allow, Allow]), Allow);
        assert_eq!(combined(&[Allow, Ask]), Ask);
        assert_eq!(combined(&[Ask, Block]), Block);
        assert_eq!(combined(&[]), Block);
    }

    #[test]
    fn a_site_that_asks_is_listed_and_keeps_each_choice() {
        let store = Store::in_memory();
        let site = "https://meet.test";
        assert_eq!(
            store.site_asked(site, Device::Camera, 5).unwrap(),
            SitePermission::Ask
        );
        store
            .set_site(site, Device::Camera, SitePermission::Allow)
            .unwrap();
        store
            .set_site(site, Device::Microphone, SitePermission::Block)
            .unwrap();
        assert_eq!(
            store.site_asked(site, Device::Camera, 6).unwrap(),
            SitePermission::Allow
        );
        assert_eq!(
            store.site_asked(site, Device::Microphone, 7).unwrap(),
            SitePermission::Block
        );
        assert_eq!(
            store.sites().unwrap(),
            [BrowserSite {
                origin: site.into(),
                camera: SitePermission::Allow,
                microphone: SitePermission::Block,
                asked_ms: 7,
            }]
        );
        store.clear_sites(Some(8)).unwrap();
        assert_eq!(store.sites().unwrap().len(), 1);
        store.clear_sites(None).unwrap();
        assert!(store.sites().unwrap().is_empty());
    }
}
