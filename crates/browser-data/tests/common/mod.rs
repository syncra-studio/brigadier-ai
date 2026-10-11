//! Fixture builders shared by the integration tests: fake profile databases built with rusqlite,
//! Chromium's cookie encryption, and an encoder for Safari's binarycookies layout.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use aes::cipher::block_padding::Pkcs7;
use aes::cipher::{BlockModeEncrypt, KeyIvInit};
use brigadier_browser_data::{Family, Profile};
use rusqlite::Connection;
use sha2::{Digest, Sha256};

/// A fixed "now" for expiry checks: 2026-10-11T00:00:00Z.
pub const NOW: i64 = 1_791_676_800;
pub const PASSWORD: &[u8] = b"test-password";

/// Unix seconds → Chromium's microseconds since 1601.
pub fn chromium_time(unix_secs: i64) -> i64 {
    (unix_secs + 11_644_473_600) * 1_000_000
}

/// Encrypts a cookie value the way Chromium on macOS does, with the SHA-256 of the domain in
/// front when `hash_host` is given (cookie store version 24 and later).
pub fn chromium_encrypt(password: &[u8], value: &str, hash_host: Option<&str>) -> Vec<u8> {
    let key = pbkdf2::pbkdf2_hmac_array::<sha1::Sha1, 16>(password, b"saltysalt", 1003);
    let mut plaintext = Vec::new();
    if let Some(host) = hash_host {
        plaintext.extend_from_slice(&Sha256::digest(host.as_bytes()));
    }
    plaintext.extend_from_slice(value.as_bytes());
    let ciphertext = cbc::Encryptor::<aes::Aes128>::new(&key.into(), &[b' '; 16].into())
        .encrypt_padded_vec::<Pkcs7>(&plaintext);
    [b"v10".as_slice(), &ciphertext].concat()
}

pub fn mkdirs(path: &Path) {
    std::fs::create_dir_all(path).unwrap();
}

/// Writes a placeholder file (discovery only checks the files open).
pub fn touch(path: &Path) {
    mkdirs(path.parent().unwrap());
    std::fs::write(path, b"x").unwrap();
}

/// A current-shape Chromium cookie store at `version`.
pub fn chromium_cookies_db(path: &Path, version: i64) -> Connection {
    mkdirs(path.parent().unwrap());
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE meta(key LONGVARCHAR NOT NULL UNIQUE PRIMARY KEY, value LONGVARCHAR);
         CREATE TABLE cookies(creation_utc INTEGER NOT NULL, host_key TEXT NOT NULL,
           top_frame_site_key TEXT NOT NULL, name TEXT NOT NULL, value TEXT NOT NULL,
           encrypted_value BLOB NOT NULL, path TEXT NOT NULL, expires_utc INTEGER NOT NULL,
           is_secure INTEGER NOT NULL, is_httponly INTEGER NOT NULL,
           last_access_utc INTEGER NOT NULL, has_expires INTEGER NOT NULL,
           is_persistent INTEGER NOT NULL, priority INTEGER NOT NULL, samesite INTEGER NOT NULL,
           source_scheme INTEGER NOT NULL, source_port INTEGER NOT NULL,
           last_update_utc INTEGER NOT NULL, source_type INTEGER NOT NULL,
           has_cross_site_ancestor INTEGER NOT NULL);",
    )
    .unwrap();
    conn.execute("INSERT INTO meta VALUES ('version', ?1)", [version])
        .unwrap();
    conn
}

/// One row for [`chromium_cookies_db`].
pub struct ChromiumRow<'a> {
    pub host: &'a str,
    pub top_frame: &'a str,
    pub name: &'a str,
    pub value: &'a str,
    pub encrypted: Vec<u8>,
    pub path: &'a str,
    pub expires_unix: i64,
    pub secure: bool,
    pub http_only: bool,
    pub persistent: bool,
    pub samesite: i64,
}

impl Default for ChromiumRow<'_> {
    fn default() -> Self {
        Self {
            host: ".example.com",
            top_frame: "",
            name: "n",
            value: "",
            encrypted: Vec::new(),
            path: "/",
            expires_unix: NOW + 86_400,
            secure: false,
            http_only: false,
            persistent: true,
            samesite: -1,
        }
    }
}

pub fn insert_chromium(conn: &Connection, row: &ChromiumRow<'_>) {
    let expires = if row.persistent {
        chromium_time(row.expires_unix)
    } else {
        0
    };
    conn.execute(
        "INSERT INTO cookies VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?1, ?11, ?11, 1,
           ?12, 2, 443, ?1, 0, 0)",
        rusqlite::params![
            chromium_time(NOW - 1000),
            row.host,
            row.top_frame,
            row.name,
            row.value,
            row.encrypted,
            row.path,
            expires,
            row.secure,
            row.http_only,
            row.persistent,
            row.samesite,
        ],
    )
    .unwrap();
}

pub fn chromium_history_db(path: &Path) -> Connection {
    mkdirs(path.parent().unwrap());
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE urls(id INTEGER PRIMARY KEY AUTOINCREMENT, url LONGVARCHAR, title LONGVARCHAR,
           visit_count INTEGER DEFAULT 0 NOT NULL, typed_count INTEGER DEFAULT 0 NOT NULL,
           last_visit_time INTEGER NOT NULL, hidden INTEGER DEFAULT 0 NOT NULL);
         CREATE TABLE visits(id INTEGER PRIMARY KEY, url INTEGER NOT NULL,
           visit_time INTEGER NOT NULL, from_visit INTEGER, transition INTEGER DEFAULT 0 NOT NULL);",
    )
    .unwrap();
    conn
}

pub fn firefox_cookies_db(path: &Path, user_version: i64, partitioned_column: bool) -> Connection {
    mkdirs(path.parent().unwrap());
    let conn = Connection::open(path).unwrap();
    let extra = if partitioned_column {
        ", isPartitionedAttributeSet INTEGER DEFAULT 0"
    } else {
        ""
    };
    conn.execute_batch(&format!(
        "PRAGMA journal_mode = WAL;
         CREATE TABLE moz_cookies (id INTEGER PRIMARY KEY, originAttributes TEXT NOT NULL DEFAULT '',
           name TEXT, value TEXT, host TEXT, path TEXT, expiry INTEGER, lastAccessed INTEGER,
           creationTime INTEGER, isSecure INTEGER, isHttpOnly INTEGER,
           inBrowserElement INTEGER DEFAULT 0, sameSite INTEGER DEFAULT 0,
           rawSameSite INTEGER DEFAULT 0, schemeMap INTEGER DEFAULT 0{extra},
           CONSTRAINT moz_uniqueid UNIQUE (name, host, path, originAttributes));
         PRAGMA user_version = {user_version};"
    ))
    .unwrap();
    conn
}

pub fn firefox_places_db(path: &Path) -> Connection {
    mkdirs(path.parent().unwrap());
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         CREATE TABLE moz_places (id INTEGER PRIMARY KEY, url LONGVARCHAR, title LONGVARCHAR,
           rev_host LONGVARCHAR, visit_count INTEGER DEFAULT 0, last_visit_date INTEGER);
         CREATE TABLE moz_historyvisits (id INTEGER PRIMARY KEY, from_visit INTEGER,
           place_id INTEGER, visit_date INTEGER, visit_type INTEGER, session INTEGER);",
    )
    .unwrap();
    conn
}

pub fn safari_history_db(path: &Path) -> Connection {
    mkdirs(path.parent().unwrap());
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE history_items (id INTEGER PRIMARY KEY AUTOINCREMENT, url TEXT NOT NULL UNIQUE,
           domain_expansion TEXT NULL, visit_count INTEGER NOT NULL DEFAULT 0);
         CREATE TABLE history_visits (id INTEGER PRIMARY KEY AUTOINCREMENT,
           history_item INTEGER NOT NULL, visit_time REAL NOT NULL, title TEXT NULL,
           load_successful BOOLEAN NOT NULL DEFAULT 1);",
    )
    .unwrap();
    conn
}

/// One cookie for [`encode_binarycookies`]. `expiry` is seconds since 2001-01-01.
pub struct BinCookie<'a> {
    pub domain: &'a str,
    pub name: &'a str,
    pub path: &'a str,
    pub value: &'a str,
    pub flags: u32,
    pub expiry: f64,
}

/// Encodes pages of cookies in Safari's classic `Cookies.binarycookies` layout.
pub fn encode_binarycookies(pages: &[Vec<BinCookie<'_>>]) -> Vec<u8> {
    let encoded: Vec<Vec<u8>> = pages.iter().map(|cookies| encode_page(cookies)).collect();
    let mut file = b"cook".to_vec();
    file.extend_from_slice(&(encoded.len() as u32).to_be_bytes());
    for page in &encoded {
        file.extend_from_slice(&(page.len() as u32).to_be_bytes());
    }
    for page in &encoded {
        file.extend_from_slice(page);
    }
    // Checksum and footer, which readers ignore.
    file.extend_from_slice(&0u32.to_be_bytes());
    file.extend_from_slice(&0x0717_2005_0000_004b_u64.to_be_bytes());
    file
}

fn encode_page(cookies: &[BinCookie<'_>]) -> Vec<u8> {
    let records: Vec<Vec<u8>> = cookies.iter().map(encode_record).collect();
    let mut page = vec![0, 0, 1, 0];
    page.extend_from_slice(&(records.len() as u32).to_le_bytes());
    let mut offset = 8 + 4 * records.len() + 4;
    for record in &records {
        page.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += record.len();
    }
    page.extend_from_slice(&[0; 4]);
    for record in &records {
        page.extend_from_slice(record);
    }
    page
}

fn encode_record(cookie: &BinCookie<'_>) -> Vec<u8> {
    let mut strings = Vec::new();
    let mut offsets = Vec::new();
    for s in [cookie.domain, cookie.name, cookie.path, cookie.value] {
        offsets.push(56 + strings.len());
        strings.extend_from_slice(s.as_bytes());
        strings.push(0);
    }
    let size = 56 + strings.len();
    let mut record = Vec::new();
    for word in [
        size,
        0,
        cookie.flags as usize,
        0,
        offsets[0],
        offsets[1],
        offsets[2],
        offsets[3],
    ] {
        record.extend_from_slice(&(word as u32).to_le_bytes());
    }
    record.extend_from_slice(&[0; 8]);
    record.extend_from_slice(&cookie.expiry.to_le_bytes());
    record.extend_from_slice(&(cookie.expiry - 86_400.0).to_le_bytes());
    record.extend_from_slice(&strings);
    record
}

/// Unix seconds → Safari's seconds since 2001.
pub fn apple_time(unix_secs: i64) -> f64 {
    (unix_secs - 978_307_200) as f64
}

/// A bare profile pointing at `dir`, for reading fixtures without discovery.
pub fn profile(id: &str, family: Family, dir: PathBuf) -> Profile {
    Profile {
        id: id.to_owned(),
        browser: "Test".to_owned(),
        name: "Default".to_owned(),
        family,
        dir,
        app_path: None,
        keychain_service: None,
        keychain_account: None,
        has_cookies: true,
        has_history: true,
        readable: true,
        is_default: true,
    }
}
