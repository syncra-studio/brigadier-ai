//! Saved passwords, read once so an import can seal them into Brigadier's own store.
//!
//! A [`Password`] has no `Serialize`, and its `Debug` hides it, so no log line or derived reply can
//! carry one by accident. [`Password::reveal`] is the one way out, for the shell's import to hand
//! it to the profile's vault.
//!
//! - **Chromium** keeps them in `Login Data` and `Login Data For Account`, sealed as its cookies
//!   are. `v20` values stay closed, as for cookies.
//! - **Firefox** keeps them in `logins.json`, sealed with a key in `key4.db`, itself sealed with
//!   the user's Primary Password. With one set this crate can't open them and says so; it never
//!   asks for it or guesses it.
//! - **Safari** keeps them in iCloud Keychain, which no other app can read.

use std::collections::HashSet;
use std::path::Path;

use aes::cipher::block_padding::Pkcs7;
use aes::cipher::{BlockModeDecrypt, KeyIvInit};
use base64::Engine;
use hmac::{Hmac, KeyInit, Mac};
use rusqlite::Connection;
use sha1::{Digest, Sha1};
use sha2::Sha256;

use crate::chromium::{Opened, open_sealed};
use crate::sqlite::{columns, io_error, open_copy};
use crate::{
    ChromiumKeys, Error, Family, Profile, SecretError, WEBKIT_EPOCH_OFFSET_SECS, is_web_url,
};

/// A saved password. Its value stays inside this crate; see the module's notes.
#[derive(Clone, PartialEq, Eq)]
pub struct Password(String);

impl Password {
    pub(crate) fn new(value: String) -> Self {
        Self(value)
    }

    /// The value itself, for the vault it is going into and nothing else.
    pub fn reveal(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Password {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Password(hidden)")
    }
}

/// One saved sign-in, in Chromium's terms, which any engine's can be put into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credential {
    /// The sign-in page, e.g. `https://example.com/login`.
    pub origin_url: String,
    /// Where the form posts; empty when unknown.
    pub action_url: String,
    /// What the password is for: `https://example.com/`, plus a realm for HTTP auth.
    pub signon_realm: String,
    pub username_element: String,
    pub username: String,
    pub password_element: String,
    pub password: Password,
    /// Chromium's scheme: 0 for a web form, 1 for HTTP basic and 2 for digest.
    pub scheme: i64,
    pub created_at_ms: Option<i64>,
    pub last_used_at_ms: Option<i64>,
}

/// The passwords read from one profile, plus what was left out and why.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Passwords {
    pub credentials: Vec<Credential>,
    /// Couldn't be opened, or malformed.
    pub unreadable: usize,
    /// The sites of those, one per password, when known. Never a username or a value.
    pub unreadable_sites: Vec<String>,
    /// Chrome's `v20` values among the unreadable, which only that browser can open.
    pub app_bound: usize,
    /// Not for a web page (`android://`, an extension's), or a site the user said never to save.
    pub skipped: usize,
}

/// Whether a profile has any saved password, opening nothing sealed.
pub(crate) fn has_saved(profile: &Profile) -> bool {
    match profile.family {
        Family::Chromium => ["Login Data", "Login Data For Account"].iter().any(|file| {
            open_copy(&profile.dir.join(file)).is_ok_and(|conn| {
                conn.query_row("SELECT 1 FROM logins LIMIT 1", [], |_| Ok(()))
                    .is_ok()
            })
        }),
        Family::Gecko => std::fs::read(profile.dir.join("logins.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .and_then(|logins| logins.get("logins")?.as_array().map(|all| !all.is_empty()))
            .unwrap_or(false),
        Family::Safari => false,
    }
}

/// Reads a profile's saved passwords, http and https sites only. `keys` come from
/// [`crate::chromium_keys`] and are only used for Chromium.
pub fn read_passwords(profile: &Profile, keys: &ChromiumKeys) -> Result<Passwords, Error> {
    match profile.family {
        Family::Chromium => read_chromium(&profile.dir, keys),
        Family::Gecko => read_gecko(&profile.dir),
        Family::Safari => Err(Error::NotReadable(
            "Safari keeps passwords in iCloud Keychain",
        )),
    }
}

fn read_chromium(dir: &Path, keys: &ChromiumKeys) -> Result<Passwords, Error> {
    let mut out = Passwords::default();
    let mut found = false;
    for file in ["Login Data", "Login Data For Account"] {
        match open_copy(&dir.join(file)) {
            Ok(conn) => {
                found = true;
                read_login_db(&conn, keys, &mut out)?;
            }
            Err(Error::NotFound(_)) => {}
            Err(error) => return Err(error),
        }
    }
    if !found {
        return Err(Error::NotFound(dir.join("Login Data")));
    }
    // The same sign-in in both stores counts once; the first store wins.
    let mut seen = HashSet::new();
    out.credentials.retain(|c| {
        seen.insert((
            c.origin_url.clone(),
            c.username_element.clone(),
            c.username.clone(),
            c.password_element.clone(),
            c.signon_realm.clone(),
        ))
    });
    Ok(out)
}

fn read_login_db(conn: &Connection, keys: &ChromiumKeys, out: &mut Passwords) -> Result<(), Error> {
    let cols = columns(conn, "logins")?;
    if !cols.contains("signon_realm") || !cols.contains("password_value") {
        return Err(Error::Format("no Chromium logins table".to_owned()));
    }
    let optional = |name: &str, fallback: &str| {
        if cols.contains(name) {
            name.to_owned()
        } else {
            fallback.to_owned()
        }
    };
    let sql = format!(
        "SELECT origin_url, {action}, signon_realm, {user_element}, {user}, {pass_element},
                password_value, {scheme}, {created}, {used}, {blocked} FROM logins",
        action = optional("action_url", "''"),
        user_element = optional("username_element", "''"),
        user = optional("username_value", "''"),
        pass_element = optional("password_element", "''"),
        scheme = optional("scheme", "0"),
        created = optional("date_created", "0"),
        used = optional("date_last_used", "0"),
        blocked = optional("blacklisted_by_user", "0"),
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let origin_url: String = row.get::<_, Option<String>>(0)?.unwrap_or_default();
        let signon_realm: String = row.get::<_, Option<String>>(2)?.unwrap_or_default();
        let blocked = row.get::<_, Option<i64>>(10)?.unwrap_or(0) != 0;
        if blocked || !is_web_url(&signon_realm) {
            out.skipped += 1;
            continue;
        }
        let sealed: Vec<u8> = row.get::<_, Option<Vec<u8>>>(6)?.unwrap_or_default();
        let password = match open_sealed(&sealed, keys, &|bytes| String::from_utf8(bytes).ok()) {
            Opened::Value(password) => password,
            Opened::AppBound => {
                out.unreadable += 1;
                out.app_bound += 1;
                out.unreadable_sites.push(signon_realm);
                continue;
            }
            Opened::Unreadable => {
                out.unreadable += 1;
                out.unreadable_sites.push(signon_realm);
                continue;
            }
        };
        out.credentials.push(Credential {
            origin_url,
            action_url: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
            signon_realm,
            username_element: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
            username: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
            password_element: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
            password: Password::new(password),
            scheme: row.get::<_, Option<i64>>(7)?.unwrap_or(0),
            created_at_ms: chromium_ms(row.get::<_, Option<i64>>(8)?),
            last_used_at_ms: chromium_ms(row.get::<_, Option<i64>>(9)?),
        });
    }
    Ok(())
}

/// Chromium's microseconds since 1601 in Unix milliseconds; 0 means unrecorded.
fn chromium_ms(micros: Option<i64>) -> Option<i64> {
    micros
        .filter(|micros| *micros > 0)
        .map(|micros| micros.div_euclid(1000) - WEBKIT_EPOCH_OFFSET_SECS * 1000)
}

const OID_PBES2: &[u8] = &[0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x05, 0x0D];
const OID_PBE_SHA1_3DES: &[u8] = &[
    0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x0C, 0x05, 0x01, 0x03,
];
const OID_HMAC_SHA256: &[u8] = &[0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x02, 0x09];
const OID_3DES_CBC: &[u8] = &[0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x03, 0x07];
const OID_AES256_CBC: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x01, 0x2A];
/// What key4.db's check value opens to when the password is right (here, empty).
const PASSWORD_CHECK: &[u8] = b"password-check";

/// One DER value: its tag, its contents, and what follows it.
fn der(data: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (&tag, rest) = data.split_first()?;
    let (&first, rest) = rest.split_first()?;
    let (len, rest) = if first < 0x80 {
        (usize::from(first), rest)
    } else {
        let count = usize::from(first & 0x7F);
        if count == 0 || count > 4 {
            return None;
        }
        let (bytes, rest) = rest.split_at_checked(count)?;
        let len = bytes.iter().fold(0usize, |n, b| (n << 8) | usize::from(*b));
        (len, rest)
    };
    let (content, rest) = rest.split_at_checked(len)?;
    Some((tag, content, rest))
}

/// The next DER value if it has `tag`, and what follows it.
fn take(data: &[u8], tag: u8) -> Option<(&[u8], &[u8])> {
    let (found, content, rest) = der(data)?;
    (found == tag).then_some((content, rest))
}

fn der_uint(content: &[u8]) -> Option<u32> {
    if content.len() > 4 {
        return None;
    }
    Some(content.iter().fold(0u32, |n, b| (n << 8) | u32::from(*b)))
}

const SEQUENCE: u8 = 0x30;
const OCTETS: u8 = 0x04;
const OID: u8 = 0x06;
const INTEGER: u8 = 0x02;

fn aes256_cbc(key: &[u8], iv: &[u8], data: &[u8]) -> Option<Vec<u8>> {
    let decryptor = cbc::Decryptor::<aes::Aes256>::new_from_slices(key, iv).ok()?;
    decryptor.decrypt_padded_vec::<Pkcs7>(data).ok()
}

fn des3_cbc(key: &[u8], iv: &[u8], data: &[u8]) -> Option<Vec<u8>> {
    let decryptor = cbc::Decryptor::<des::TdesEde3>::new_from_slices(key, iv).ok()?;
    decryptor.decrypt_padded_vec::<Pkcs7>(data).ok()
}

/// Opens a value of key4.db, sealed with a key from the global salt and the empty password.
fn open_nss_pbe(blob: &[u8], global_salt: &[u8]) -> Option<Vec<u8>> {
    let (outer, _) = take(blob, SEQUENCE)?;
    let (algorithm, rest) = take(outer, SEQUENCE)?;
    let (sealed, _) = take(rest, OCTETS)?;
    let (oid, params) = take(algorithm, OID)?;
    // The password is empty: Firefox only uses one when the user has set a Primary Password.
    let password_hash = Sha1::digest(global_salt);
    if oid == OID_PBES2 {
        let (params, _) = take(params, SEQUENCE)?;
        let (kdf, cipher) = take(params, SEQUENCE)?;
        let (_, kdf) = take(kdf, OID)?;
        let (kdf, _) = take(kdf, SEQUENCE)?;
        let (salt, kdf) = take(kdf, OCTETS)?;
        let (iterations, kdf) = take(kdf, INTEGER)?;
        let (_, kdf) = take(kdf, INTEGER)?;
        let (hash, _) = take(kdf, SEQUENCE)?;
        let (hash, _) = take(hash, OID)?;
        if hash != OID_HMAC_SHA256 {
            return None;
        }
        let (cipher, _) = take(cipher, SEQUENCE)?;
        let (_, cipher) = take(cipher, OID)?;
        let (iv_tail, _) = take(cipher, OCTETS)?;
        let key = pbkdf2::pbkdf2_hmac_array::<Sha256, 32>(
            password_hash.as_slice(),
            salt,
            der_uint(iterations)?,
        );
        // NSS stores only the IV's last 14 bytes, the first two being its own DER header.
        let iv = [&[0x04, 0x0E], iv_tail].concat();
        return aes256_cbc(&key, &iv, sealed);
    }
    if oid == OID_PBE_SHA1_3DES {
        let (params, _) = take(params, SEQUENCE)?;
        let (salt, _) = take(params, OCTETS)?;
        let chp = Sha1::digest([password_hash.as_slice(), salt].concat());
        let mut padded = salt.to_vec();
        padded.resize(20.max(salt.len()), 0);
        let hmac = |parts: &[&[u8]]| -> Option<Vec<u8>> {
            let mut mac = <Hmac<Sha1> as KeyInit>::new_from_slice(chp.as_slice()).ok()?;
            for part in parts {
                mac.update(part);
            }
            Some(mac.finalize().into_bytes().to_vec())
        };
        let k1 = hmac(&[&padded, salt])?;
        let tk = hmac(&[&padded])?;
        let k2 = hmac(&[&tk, salt])?;
        let k = [k1, k2].concat();
        return des3_cbc(k.get(..24)?, k.get(k.len() - 8..)?, sealed);
    }
    None
}

/// The keys that open logins.json, each with the id its logins name it by.
type GeckoKeys = Vec<(Vec<u8>, Vec<u8>)>;

fn gecko_keys(conn: &Connection) -> Result<GeckoKeys, Error> {
    let bad = || Error::Format("key4.db".to_owned());
    let (global_salt, check): (Vec<u8>, Vec<u8>) = conn
        .query_row(
            "SELECT item1, item2 FROM metaData WHERE id = 'password'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|_| bad())?;
    let opened = open_nss_pbe(&check, &global_salt);
    if !opened.is_some_and(|plain| plain.starts_with(PASSWORD_CHECK)) {
        // Only a Primary Password makes the empty one fail.
        return Err(Error::Secret(SecretError::Locked));
    }
    let mut stmt = conn.prepare("SELECT a102, a11 FROM nssPrivate")?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, Option<Vec<u8>>>(0)?,
            row.get::<_, Option<Vec<u8>>>(1)?,
        ))
    })?;
    let mut keys = Vec::new();
    for row in rows {
        let (id, sealed) = row?;
        let Some(sealed) = sealed else { continue };
        if let Some(key) = open_nss_pbe(&sealed, &global_salt) {
            keys.push((id.unwrap_or_default(), key));
        }
    }
    if keys.is_empty() {
        return Err(bad());
    }
    Ok(keys)
}

/// Opens one of logins.json's fields: base64 of a key id, a cipher and its IV, and the sealed text.
fn open_login_field(field: &str, keys: &GeckoKeys) -> Option<String> {
    let blob = base64::engine::general_purpose::STANDARD
        .decode(field)
        .ok()?;
    let (outer, _) = take(&blob, SEQUENCE)?;
    let (key_id, rest) = take(outer, OCTETS)?;
    let (cipher, rest) = take(rest, SEQUENCE)?;
    let (sealed, _) = take(rest, OCTETS)?;
    let (oid, cipher) = take(cipher, OID)?;
    let (iv, _) = take(cipher, OCTETS)?;
    let key = keys
        .iter()
        .find(|(id, _)| id == key_id)
        .or_else(|| keys.first())
        .map(|(_, key)| key)?;
    let plain = if oid == OID_3DES_CBC {
        des3_cbc(key.get(..24)?, iv, sealed)?
    } else if oid == OID_AES256_CBC {
        aes256_cbc(key.get(..32)?, iv, sealed)?
    } else {
        return None;
    };
    String::from_utf8(plain).ok()
}

fn read_gecko(dir: &Path) -> Result<Passwords, Error> {
    let path = dir.join("logins.json");
    let text = std::fs::read(&path).map_err(|err| io_error(&path, err))?;
    let logins: serde_json::Value = serde_json::from_slice(&text)
        .map_err(|err| Error::Format(format!("logins.json: {err}")))?;
    let keys = gecko_keys(&open_copy(&dir.join("key4.db"))?)?;
    let text_at = |login: &serde_json::Value, name: &str| {
        login
            .get(name)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let ms_at = |login: &serde_json::Value, name: &str| {
        login
            .get(name)
            .and_then(serde_json::Value::as_i64)
            .filter(|ms| *ms > 0)
    };
    let mut out = Passwords::default();
    let entries = logins.get("logins").and_then(serde_json::Value::as_array);
    for login in entries.into_iter().flatten() {
        let hostname = text_at(login, "hostname");
        if !is_web_url(&hostname) {
            out.skipped += 1;
            continue;
        }
        let opened = (
            open_login_field(&text_at(login, "encryptedUsername"), &keys),
            open_login_field(&text_at(login, "encryptedPassword"), &keys),
        );
        let (Some(username), Some(password)) = opened else {
            out.unreadable += 1;
            out.unreadable_sites.push(hostname);
            continue;
        };
        let realm = text_at(login, "httpRealm");
        let origin = hostname.trim_end_matches('/');
        let signon_realm = if realm.is_empty() {
            format!("{origin}/")
        } else {
            format!("{origin}/{realm}")
        };
        out.credentials.push(Credential {
            origin_url: format!("{origin}/"),
            action_url: text_at(login, "formSubmitURL"),
            signon_realm,
            username_element: text_at(login, "usernameField"),
            username,
            password_element: text_at(login, "passwordField"),
            password: Password::new(password),
            // Firefox doesn't record which HTTP scheme; Chromium's basic works for matching.
            scheme: i64::from(!realm.is_empty()),
            created_at_ms: ms_at(login, "timeCreated"),
            last_used_at_ms: ms_at(login, "timeLastUsed"),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use aes::cipher::BlockModeEncrypt;

    use super::*;

    fn tlv(tag: u8, parts: &[&[u8]]) -> Vec<u8> {
        let body = parts.concat();
        let mut out = vec![tag];
        if body.len() < 0x80 {
            out.push(u8::try_from(body.len()).unwrap());
        } else {
            out.push(0x82);
            out.extend_from_slice(&u16::try_from(body.len()).unwrap().to_be_bytes());
        }
        out.extend(body);
        out
    }

    fn seal_aes(key: &[u8], iv: &[u8], plain: &[u8]) -> Vec<u8> {
        cbc::Encryptor::<aes::Aes256>::new_from_slices(key, iv)
            .unwrap()
            .encrypt_padded_vec::<Pkcs7>(plain)
    }

    fn seal_3des(key: &[u8], iv: &[u8], plain: &[u8]) -> Vec<u8> {
        cbc::Encryptor::<des::TdesEde3>::new_from_slices(key, iv)
            .unwrap()
            .encrypt_padded_vec::<Pkcs7>(plain)
    }

    /// A key4.db value in the PBES2 form current Firefox writes.
    fn pbes2(global_salt: &[u8], password: &[u8], plain: &[u8]) -> Vec<u8> {
        let salt = [5u8; 32];
        let hash = Sha1::digest([global_salt, password].concat());
        let key = pbkdf2::pbkdf2_hmac_array::<Sha256, 32>(hash.as_slice(), &salt, 10);
        let iv_tail = [6u8; 14];
        let iv = [&[0x04, 0x0E], iv_tail.as_slice()].concat();
        let sealed = seal_aes(&key, &iv, plain);
        tlv(
            SEQUENCE,
            &[
                &tlv(
                    SEQUENCE,
                    &[
                        &tlv(OID, &[OID_PBES2]),
                        &tlv(
                            SEQUENCE,
                            &[
                                &tlv(
                                    SEQUENCE,
                                    &[
                                        &tlv(
                                            OID,
                                            &[&[
                                                0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x05,
                                                0x0C,
                                            ]],
                                        ),
                                        &tlv(
                                            SEQUENCE,
                                            &[
                                                &tlv(OCTETS, &[&salt]),
                                                &tlv(INTEGER, &[&[10]]),
                                                &tlv(INTEGER, &[&[32]]),
                                                &tlv(SEQUENCE, &[&tlv(OID, &[OID_HMAC_SHA256])]),
                                            ],
                                        ),
                                    ],
                                ),
                                &tlv(
                                    SEQUENCE,
                                    &[&tlv(OID, &[OID_AES256_CBC]), &tlv(OCTETS, &[&iv_tail])],
                                ),
                            ],
                        ),
                    ],
                ),
                &tlv(OCTETS, &[&sealed]),
            ],
        )
    }

    /// A key4.db value in the older PBE-SHA1-3DES form, which long-lived profiles still have.
    fn legacy_pbe(global_salt: &[u8], plain: &[u8]) -> Vec<u8> {
        let salt = [9u8; 20];
        let hp = Sha1::digest(global_salt);
        let chp = Sha1::digest([hp.as_slice(), &salt].concat());
        let hmac = |parts: &[&[u8]]| {
            let mut mac = <Hmac<Sha1> as KeyInit>::new_from_slice(chp.as_slice()).unwrap();
            for part in parts {
                mac.update(part);
            }
            mac.finalize().into_bytes().to_vec()
        };
        let k1 = hmac(&[&salt, &salt]);
        let tk = hmac(&[&salt]);
        let k2 = hmac(&[&tk, &salt]);
        let k = [k1, k2].concat();
        let sealed = seal_3des(&k[..24], &k[k.len() - 8..], plain);
        tlv(
            SEQUENCE,
            &[
                &tlv(
                    SEQUENCE,
                    &[
                        &tlv(OID, &[OID_PBE_SHA1_3DES]),
                        &tlv(SEQUENCE, &[&tlv(OCTETS, &[&salt]), &tlv(INTEGER, &[&[1]])]),
                    ],
                ),
                &tlv(OCTETS, &[&sealed]),
            ],
        )
    }

    fn login_field(key_id: &[u8], oid: &[u8], iv: &[u8], sealed: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(tlv(
            SEQUENCE,
            &[
                &tlv(OCTETS, &[key_id]),
                &tlv(SEQUENCE, &[&tlv(OID, &[oid]), &tlv(OCTETS, &[iv])]),
                &tlv(OCTETS, &[sealed]),
            ],
        ))
    }

    fn key4(dir: &Path, check: &[u8], private: &[u8], global_salt: &[u8]) {
        let conn = Connection::open(dir.join("key4.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE metaData (id PRIMARY KEY, item1, item2);
             CREATE TABLE nssPrivate (a11 BLOB, a102 BLOB);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO metaData VALUES ('password', ?1, ?2)",
            rusqlite::params![global_salt, check],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO nssPrivate VALUES (?1, ?2)",
            rusqlite::params![private, b"key-id".as_slice()],
        )
        .unwrap();
    }

    fn profile(family: Family, dir: &Path) -> Profile {
        Profile {
            id: "x/y".to_owned(),
            browser: "Test".to_owned(),
            name: "Default".to_owned(),
            family,
            dir: dir.to_owned(),
            app_path: None,
            keychain_service: None,
            keychain_account: None,
            local_state: None,
            linux_secret_application: None,
            has_cookies: false,
            has_history: false,
            has_bookmarks: false,
            readable: true,
            is_default: true,
        }
    }

    fn logins_json(entries: &str) -> String {
        format!(r#"{{"nextId":3,"logins":[{entries}]}}"#)
    }

    fn firefox_fixture(dir: &Path, check: &[u8], private: &[u8], salt: &[u8], key: &[u8]) {
        key4(dir, check, private, salt);
        let iv8 = [3u8; 8];
        let iv16 = [4u8; 16];
        let user = login_field(
            b"key-id",
            OID_3DES_CBC,
            &iv8,
            &seal_3des(&key[..24], &iv8, b"me"),
        );
        let pass = login_field(
            b"key-id",
            OID_3DES_CBC,
            &iv8,
            &seal_3des(&key[..24], &iv8, b"s3cret"),
        );
        let aes_user = key.get(..32).map(|key| {
            login_field(
                b"key-id",
                OID_AES256_CBC,
                &iv16,
                &seal_aes(key, &iv16, b"you"),
            )
        });
        let aes_pass = key.get(..32).map(|key| {
            login_field(
                b"key-id",
                OID_AES256_CBC,
                &iv16,
                &seal_aes(key, &iv16, b"h1dden"),
            )
        });
        let mut entries = vec![format!(
            r#"{{"hostname":"https://a.example","httpRealm":null,"formSubmitURL":"https://a.example/in",
                "usernameField":"u","passwordField":"p","encryptedUsername":"{user}",
                "encryptedPassword":"{pass}","timeCreated":1000,"timeLastUsed":2000}}"#
        )];
        if let (Some(user), Some(pass)) = (aes_user, aes_pass) {
            entries.push(format!(
                r#"{{"hostname":"https://auth.example","httpRealm":"Staff","encryptedUsername":"{user}",
                    "encryptedPassword":"{pass}"}}"#
            ));
        }
        entries.push(format!(
            r#"{{"hostname":"chrome://FirefoxAccounts","encryptedUsername":"{user}","encryptedPassword":"{pass}"}}"#
        ));
        entries.push(
            r#"{"hostname":"https://bad.example","encryptedUsername":"***","encryptedPassword":"***"}"#
                .to_owned(),
        );
        std::fs::write(dir.join("logins.json"), logins_json(&entries.join(","))).unwrap();
    }

    #[test]
    fn firefox_opens_both_ciphers_under_a_current_key_store() {
        let dir = tempfile::tempdir().unwrap();
        let salt = [1u8; 20];
        let key = [8u8; 32];
        firefox_fixture(
            dir.path(),
            &pbes2(&salt, b"", b"password-check"),
            &pbes2(&salt, b"", &key),
            &salt,
            &key,
        );
        let read = read_passwords(
            &profile(Family::Gecko, dir.path()),
            &ChromiumKeys::default(),
        )
        .unwrap();
        let [form, http] = read.credentials.as_slice() else {
            panic!("two credentials expected: {read:?}");
        };
        assert_eq!(form.username, "me");
        assert_eq!(form.password.reveal(), "s3cret");
        assert_eq!(form.origin_url, "https://a.example/");
        assert_eq!(form.signon_realm, "https://a.example/");
        assert_eq!(form.action_url, "https://a.example/in");
        assert_eq!(
            (form.scheme, form.created_at_ms, form.last_used_at_ms),
            (0, Some(1000), Some(2000))
        );
        assert_eq!(http.password.reveal(), "h1dden");
        assert_eq!(http.signon_realm, "https://auth.example/Staff");
        assert_eq!(http.scheme, 1);
        assert_eq!(read.skipped, 1);
        assert_eq!(read.unreadable, 1);
        assert_eq!(read.unreadable_sites, ["https://bad.example"]);
    }

    #[test]
    fn firefox_opens_the_legacy_key_store() {
        let dir = tempfile::tempdir().unwrap();
        let salt = [2u8; 20];
        let key = [7u8; 24];
        firefox_fixture(
            dir.path(),
            &legacy_pbe(&salt, b"password-check"),
            &legacy_pbe(&salt, &key),
            &salt,
            &key,
        );
        let read = read_passwords(
            &profile(Family::Gecko, dir.path()),
            &ChromiumKeys::default(),
        )
        .unwrap();
        assert_eq!(read.credentials[0].password.reveal(), "s3cret");
    }

    #[test]
    fn a_firefox_primary_password_is_reported_never_guessed() {
        let dir = tempfile::tempdir().unwrap();
        let salt = [1u8; 20];
        let key = [8u8; 32];
        firefox_fixture(
            dir.path(),
            &pbes2(&salt, b"the primary password", b"password-check"),
            &pbes2(&salt, b"the primary password", &key),
            &salt,
            &key,
        );
        assert!(matches!(
            read_passwords(
                &profile(Family::Gecko, dir.path()),
                &ChromiumKeys::default()
            ),
            Err(Error::Secret(SecretError::Locked))
        ));
    }

    #[test]
    fn safari_says_it_cant_be_read() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            read_passwords(
                &profile(Family::Safari, dir.path()),
                &ChromiumKeys::default()
            ),
            Err(Error::NotReadable(_))
        ));
    }

    fn chromium_login_db(path: &Path) -> Connection {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE logins (origin_url VARCHAR NOT NULL, action_url VARCHAR,
               username_element VARCHAR, username_value VARCHAR, password_element VARCHAR,
               password_value BLOB, signon_realm VARCHAR NOT NULL, date_created INTEGER NOT NULL,
               blacklisted_by_user INTEGER NOT NULL, scheme INTEGER NOT NULL,
               date_last_used INTEGER NOT NULL DEFAULT 0);",
        )
        .unwrap();
        conn
    }

    fn insert_login(conn: &Connection, realm: &str, user: &str, sealed: &[u8], blocked: bool) {
        conn.execute(
            "INSERT INTO logins VALUES (?1, '', 'u', ?2, 'p', ?3, ?1, 13400000000000000, ?4, 0, 0)",
            rusqlite::params![realm, user, sealed, blocked],
        )
        .unwrap();
    }

    #[test]
    fn chromium_reads_both_stores_and_keeps_v20_closed() {
        let dir = tempfile::tempdir().unwrap();
        let keys = ChromiumKeys::from_windows_key([3; 32]);
        let one = keys.seal(b"pass-1").unwrap();
        let conn = chromium_login_db(&dir.path().join("Login Data"));
        insert_login(&conn, "https://a.example/", "me", &one, false);
        insert_login(&conn, "https://never.example/", "", b"", true);
        insert_login(&conn, "android://xyz@com.app/", "me", &one, false);
        let mut bound = keys.seal(b"locked").unwrap();
        bound[..3].copy_from_slice(b"v20");
        insert_login(&conn, "https://bound.example/", "me", &bound, false);
        insert_login(
            &conn,
            "https://wrong.example/",
            "me",
            b"v10xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
            false,
        );
        drop(conn);
        let account = chromium_login_db(&dir.path().join("Login Data For Account"));
        insert_login(
            &account,
            "https://a.example/",
            "me",
            &keys.seal(b"pass-2").unwrap(),
            false,
        );
        insert_login(
            &account,
            "https://b.example/",
            "you",
            &keys.seal(b"pass-3").unwrap(),
            false,
        );
        drop(account);

        let read = read_passwords(&profile(Family::Chromium, dir.path()), &keys).unwrap();
        let found: Vec<_> = read
            .credentials
            .iter()
            .map(|c| {
                (
                    c.signon_realm.as_str(),
                    c.username.as_str(),
                    c.password.reveal(),
                )
            })
            .collect();
        assert_eq!(
            found,
            [
                ("https://a.example/", "me", "pass-1"),
                ("https://b.example/", "you", "pass-3")
            ]
        );
        assert_eq!(read.skipped, 2);
        assert_eq!(read.unreadable, 2);
        assert_eq!(read.app_bound, 1);
        assert_eq!(
            read.unreadable_sites,
            ["https://bound.example/", "https://wrong.example/"]
        );
    }

    #[test]
    fn a_password_never_shows_in_debug_output() {
        let credential = Credential {
            origin_url: String::new(),
            action_url: String::new(),
            signon_realm: String::new(),
            username_element: String::new(),
            username: String::new(),
            password_element: String::new(),
            password: Password::new("do-not-print-me".to_owned()),
            scheme: 0,
            created_at_ms: None,
            last_used_at_ms: None,
        };
        let shown = format!(
            "{:?}",
            Passwords {
                credentials: vec![credential],
                ..Passwords::default()
            }
        );
        assert!(shown.contains("Password(hidden)"));
        assert!(!shown.contains("do-not-print-me"));
    }

    #[test]
    fn broken_der_doesnt_panic() {
        assert_eq!(der(&[]), None);
        assert_eq!(der(&[0x30, 0x85, 1, 1, 1, 1, 1]), None);
        assert_eq!(der(&[0x30, 0x05, 1]), None);
        assert!(open_nss_pbe(&[0x30, 0x02, 0x30, 0x00], b"salt").is_none());
        assert!(open_login_field("!!!", &GeckoKeys::new()).is_none());
    }
}
