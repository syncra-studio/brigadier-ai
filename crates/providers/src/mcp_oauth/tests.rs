use super::*;
use std::cell::RefCell;
use std::ffi::OsString;

#[derive(Default)]
struct FakeKeyring {
    items: RefCell<BTreeMap<(String, String), Vec<u8>>>,
    writes: RefCell<Vec<(String, String)>>,
    denied: RefCell<Option<String>>,
}
impl Keyring for FakeKeyring {
    fn get(&self, service: &str, account: &str) -> io::Result<Option<Vec<u8>>> {
        if self.denied.borrow().as_deref() == Some(service) {
            return Err(io::Error::other("fixture Keychain locked"));
        }
        Ok(self
            .items
            .borrow()
            .get(&(service.into(), account.into()))
            .cloned())
    }
    fn set(&self, service: &str, account: &str, data: &[u8]) -> io::Result<()> {
        self.writes
            .borrow_mut()
            .push((service.into(), account.into()));
        self.items
            .borrow_mut()
            .insert((service.into(), account.into()), data.to_vec());
        Ok(())
    }
    fn insert(&self, service: &str, account: &str, data: &[u8]) -> io::Result<Vec<u8>> {
        if let Some(existing) = self.get(service, account)? {
            return Ok(existing);
        }
        self.set(service, account, data)?;
        Ok(data.to_vec())
    }
}

struct Fixture {
    root: PathBuf,
    main: PathBuf,
    homes: Vec<PathBuf>,
    env: CliEnv,
    keyring: FakeKeyring,
}
impl Fixture {
    fn new(kind: ProviderKind) -> Self {
        let root =
            std::env::temp_dir().join(format!("mcp-oauth-test-{}", uuid::Uuid::new_v4().simple()));
        let main = root.join("default");
        let homes = vec![root.join("acct-one"), root.join("acct-two")];
        for home in [&main, &homes[0], &homes[1]] {
            fs::create_dir_all(home).unwrap();
        }
        let env = CliEnv::from_vars([
            (OsString::from("HOME"), root.as_os_str().to_owned()),
            (OsString::from("USER"), OsString::from("fixture")),
            (
                OsString::from(accounts::home_var(kind)),
                main.as_os_str().to_owned(),
            ),
        ]);
        Self {
            root,
            main,
            homes,
            env,
            keyring: FakeKeyring::default(),
        }
    }
    fn store(&self, kind: ProviderKind, home: &Path) -> Store {
        let env = if home == self.main {
            self.env.clone()
        } else {
            self.env.for_account(kind, home)
        };
        stores(kind, &env, home).remove(0)
    }
    fn put(&self, store: &Store, value: &Value) {
        if let Some((service, account)) = &store.keychain {
            self.keyring
                .set(service, account, &serde_json::to_vec(value).unwrap())
                .unwrap();
        } else {
            write_private(&store.file(), &serde_json::to_vec(value).unwrap()).unwrap();
        }
    }
    fn sync(&self, kind: ProviderKind) -> io::Result<()> {
        sync_with(kind, &self.env, &self.homes, &self.keyring)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn claude_token(token: &str, expiry: u64) -> Value {
    json!({"serverName":"jira", "serverUrl":"https://fixture.invalid/mcp", "accessToken":token,
        "refreshToken":format!("refresh-{token}"), "expiresAt":expiry, "clientId":"fixture-client"})
}
fn codex_token(token: &str, expiry: u64) -> Value {
    json!({"server_name":"jira", "server_url":"https://fixture.invalid/mcp", "access_token":token,
        "refresh_token":format!("refresh-{token}"), "expires_at":expiry, "client_id":"fixture-client", "scopes":["read"]})
}

#[test]
fn default_claude_sign_in_reaches_another_account_without_carrying_its_login() {
    let f = Fixture::new(ProviderKind::Claude);
    let main = f.store(ProviderKind::Claude, &f.main);
    f.put(&main, &json!({"claudeAiOauth":{"accessToken":"default-provider"}, "mcpOAuth":{"jira|fixture":claude_token("default-mcp",100)}}));
    for (i, home) in f.homes.iter().enumerate() {
        f.put(
            &f.store(ProviderKind::Claude, home),
            &json!({"claudeAiOauth":{"accessToken":format!("account-{i}")}, "other":{"kept":true}}),
        );
    }
    let original = main.read(&f.keyring).unwrap();
    f.keyring.writes.borrow_mut().clear();
    f.sync(ProviderKind::Claude).unwrap();
    for (i, home) in f.homes.iter().enumerate() {
        let got = f
            .store(ProviderKind::Claude, home)
            .read(&f.keyring)
            .unwrap();
        assert_eq!(
            got["mcpOAuth"]["jira|fixture"]["accessToken"],
            "default-mcp"
        );
        assert_eq!(got["claudeAiOauth"]["accessToken"], format!("account-{i}"));
        assert_eq!(got["other"], json!({"kept":true}));
    }
    assert_eq!(main.read(&f.keyring).unwrap(), original);
    if let Some(identity) = main.keychain {
        assert!(!f.keyring.writes.borrow().contains(&identity));
    }
    assert!(!f.main.join(".storage-write.lock").exists());
}

#[test]
fn account_sign_in_and_refresh_spread_and_a_later_stale_source_never_rolls_them_back() {
    let f = Fixture::new(ProviderKind::Claude);
    let main = f.store(ProviderKind::Claude, &f.main);
    f.put(
        &main,
        &json!({"mcpOAuth":{"jira|fixture":claude_token("old",100)}}),
    );
    f.sync(ProviderKind::Claude).unwrap();
    let one = f.store(ProviderKind::Claude, &f.homes[0]);
    f.put(&one, &json!({"claudeAiOauth":{"accessToken":"own-login"},"mcpOAuth":{
        "jira|fixture":claude_token("refresh",200), "steady|fixture":claude_token("new-sign-in",300)}}));
    f.sync(ProviderKind::Claude).unwrap();
    f.sync(ProviderKind::Claude).unwrap();
    let got = f
        .store(ProviderKind::Claude, &f.homes[1])
        .read(&f.keyring)
        .unwrap();
    assert_eq!(got["mcpOAuth"]["jira|fixture"]["accessToken"], "refresh");
    assert_eq!(
        got["mcpOAuth"]["steady|fixture"]["accessToken"],
        "new-sign-in"
    );
    assert_eq!(
        main.read(&f.keyring).unwrap()["mcpOAuth"]["jira|fixture"]["accessToken"],
        "old"
    );
    assert_eq!(
        one.read(&f.keyring).unwrap()["claudeAiOauth"]["accessToken"],
        "own-login"
    );
}

#[test]
fn empty_claude_stubs_do_not_hide_a_real_sign_in() {
    let f = Fixture::new(ProviderKind::Claude);
    f.put(
        &f.store(ProviderKind::Claude, &f.main),
        &json!({"mcpOAuth":{"jira|fixture":claude_token("signed-in",100)}}),
    );
    let own = f.store(ProviderKind::Claude, &f.homes[0]);
    f.put(&own, &json!({"mcpOAuth":{"jira|fixture":{"accessToken":"","serverUrl":"https://fixture.invalid/mcp"}}}));
    f.sync(ProviderKind::Claude).unwrap();
    assert_eq!(
        own.read(&f.keyring).unwrap()["mcpOAuth"]["jira|fixture"]["accessToken"],
        "signed-in"
    );
}

#[test]
fn unknown_and_equal_expiry_do_not_replace_existing_tokens() {
    let current = claude_token("current", 100);
    let equal = claude_token("different", 100);
    let mut unknown = claude_token("unknown", 200);
    unknown.as_object_mut().unwrap().remove("expiresAt");
    assert!(!newer(Format::Claude, &equal, Some(&current)));
    assert!(!newer(Format::Claude, &unknown, Some(&current)));
    assert!(!newer(Format::Claude, &current, Some(&unknown)));
    assert!(newer(Format::Claude, &unknown, None));
}

#[test]
fn claude_store_lock_defers_merge_until_the_cli_releases_it() {
    let f = Fixture::new(ProviderKind::Claude);
    f.put(
        &f.store(ProviderKind::Claude, &f.main),
        &json!({"mcpOAuth":{"jira|fixture":claude_token("old",100)}}),
    );
    let own = f.store(ProviderKind::Claude, &f.homes[0]);
    let lock = own.lock().unwrap().unwrap();
    f.sync(ProviderKind::Claude).unwrap();
    assert!(own.read(&f.keyring).unwrap().get("mcpOAuth").is_none());
    f.put(&own, &json!({"claudeAiOauth":{"accessToken":"just-refreshed-login"},"mcpOAuth":{"jira|fixture":claude_token("new",200)}}));
    drop(lock);
    f.sync(ProviderKind::Claude).unwrap();
    let got = own.read(&f.keyring).unwrap();
    assert_eq!(got["claudeAiOauth"]["accessToken"], "just-refreshed-login");
    assert_eq!(got["mcpOAuth"]["jira|fixture"]["accessToken"], "new");
}

#[test]
fn codex_file_sign_ins_refreshes_and_unknown_fields_are_shared_without_auth_json() {
    let f = Fixture::new(ProviderKind::Codex);
    let main = f.store(ProviderKind::Codex, &f.main);
    let old = codex_token("default", 100);
    let key = codex_key(&old).unwrap();
    f.put(&main, &json!({key.clone():old}));
    let original = fs::read(main.file()).unwrap();
    for home in &f.homes {
        fs::write(home.join("auth.json"), "fixture-provider-login").unwrap();
    }
    f.sync(ProviderKind::Codex).unwrap();
    let one = f.store(ProviderKind::Codex, &f.homes[0]);
    let mut newer = codex_token("refreshed", 200);
    newer["issuer"] = json!("https://issuer.invalid");
    f.put(&one, &json!({key.clone():newer}));
    f.sync(ProviderKind::Codex).unwrap();
    let got = f
        .store(ProviderKind::Codex, &f.homes[1])
        .read(&f.keyring)
        .unwrap();
    assert_eq!(got[&key]["access_token"], "refreshed");
    assert_eq!(got[&key]["issuer"], "https://issuer.invalid");
    assert_eq!(fs::read(main.file()).unwrap(), original);
    assert!(!f.main.join("mcp-oauth-locks").exists());
    for home in &f.homes {
        assert_eq!(
            fs::read_to_string(home.join("auth.json")).unwrap(),
            "fixture-provider-login"
        );
    }
}

#[test]
fn codex_file_lock_prevents_writing_and_then_a_refresh_wins() {
    let f = Fixture::new(ProviderKind::Codex);
    let entry = codex_token("default", 100);
    let key = codex_key(&entry).unwrap();
    f.put(
        &f.store(ProviderKind::Codex, &f.main),
        &json!({key.clone():entry}),
    );
    let own = f.store(ProviderKind::Codex, &f.homes[0]);
    let lock = own.lock().unwrap().unwrap();
    f.sync(ProviderKind::Codex).unwrap();
    assert!(!own.file().exists());
    f.put(&own, &json!({key.clone():codex_token("during-lock",200)}));
    drop(lock);
    f.sync(ProviderKind::Codex).unwrap();
    assert_eq!(
        f.store(ProviderKind::Codex, &f.homes[1])
            .read(&f.keyring)
            .unwrap()[&key]["access_token"],
        "during-lock"
    );
}

#[test]
fn encrypted_codex_namespace_uses_each_accounts_key_and_preserves_other_namespaces() {
    let f = Fixture::new(ProviderKind::Codex);
    let entry = codex_token("encrypted-default", 100);
    let key = codex_key(&entry).unwrap();
    for home in [&f.main, &f.homes[0], &f.homes[1]] {
        fs::write(
            home.join("config.toml"),
            "auth_keyring_backend = \"secrets\"\nmcp_oauth_credentials_store = \"keyring\"\n",
        )
        .unwrap();
    }
    let main = Store {
        home: f.main.clone(),
        format: Format::CodexEncrypted,
        keychain: None,
    };
    fs::create_dir_all(f.main.join("secrets")).unwrap();
    main.write(
        &main.merge(json!({}), &BTreeMap::from([(key.clone(), entry)])),
        &f.keyring,
    )
    .unwrap();
    let original = fs::read(main.file()).unwrap();
    f.keyring.writes.borrow_mut().clear();
    f.sync(ProviderKind::Codex).unwrap();
    for home in &f.homes {
        let store = Store {
            home: home.clone(),
            format: Format::CodexEncrypted,
            keychain: None,
        };
        assert_eq!(
            store.entries(&store.read(&f.keyring).unwrap())[&key]["token_response"]["access_token"],
            "encrypted-default"
        );
        assert!(!home.join(".credentials.json").exists());
        assert_ne!(
            store.passphrase(&f.keyring).unwrap(),
            main.passphrase(&f.keyring).unwrap()
        );
        fs::write(home.join("secrets/codex_auth.age"), "fixture-account-login").unwrap();
        let refreshed = codex_token("refreshed", 200);
        store
            .write(
                &store.merge(
                    store.read(&f.keyring).unwrap(),
                    &BTreeMap::from([(key.clone(), refreshed)]),
                ),
                &f.keyring,
            )
            .unwrap();
    }
    f.sync(ProviderKind::Codex).unwrap();
    for home in &f.homes {
        assert_eq!(
            fs::read_to_string(home.join("secrets/codex_auth.age")).unwrap(),
            "fixture-account-login"
        );
    }
    assert_eq!(fs::read(main.file()).unwrap(), original);
    assert!(!f.keyring.writes.borrow().iter().any(|(service,account)| service=="codex" && account==&main.key_account().unwrap()));
}

#[test]
fn corrupt_credentials_are_left_intact_and_never_exposed_in_errors() {
    let f = Fixture::new(ProviderKind::Codex);
    let own = f.store(ProviderKind::Codex, &f.homes[0]);
    fs::write(own.file(), "{ secret-fixture-not-json").unwrap();
    let error = f.sync(ProviderKind::Codex).unwrap_err().to_string();
    assert_eq!(error, "invalid MCP credential JSON");
    assert_eq!(
        fs::read_to_string(own.file()).unwrap(),
        "{ secret-fixture-not-json"
    );
}

#[cfg(unix)]
#[test]
fn symlinks_cannot_redirect_managed_writes_into_default_stores() {
    let f = Fixture::new(ProviderKind::Codex);
    let entry = codex_token("default", 100);
    f.put(
        &f.store(ProviderKind::Codex, &f.main),
        &json!({codex_key(&entry).unwrap():entry}),
    );
    let original = fs::read(f.main.join(".credentials.json")).unwrap();
    std::os::unix::fs::symlink(
        f.main.join(".credentials.json"),
        f.homes[0].join(".credentials.json"),
    )
    .unwrap();
    std::os::unix::fs::symlink(&f.main, f.homes[1].join("mcp-oauth-locks")).unwrap();
    assert!(f.sync(ProviderKind::Codex).is_err());
    assert_eq!(
        fs::read(f.main.join(".credentials.json")).unwrap(),
        original
    );
    assert!(!f.main.join("file-store.lock").exists());
}

#[test]
fn account_environment_keeps_secure_storage_separate_from_a_default_override() {
    let env = CliEnv::from_vars([(
        OsString::from("CLAUDE_SECURESTORAGE_CONFIG_DIR"),
        OsString::from("/default-storage"),
    )]);
    let account = env.for_account(ProviderKind::Claude, Path::new("/managed/acct-one"));
    assert!(account.var("CLAUDE_SECURESTORAGE_CONFIG_DIR").is_none());
    assert_eq!(
        account.var("CLAUDE_CONFIG_DIR").unwrap(),
        "/managed/acct-one"
    );
    assert_eq!(
        env.var("CLAUDE_SECURESTORAGE_CONFIG_DIR").unwrap(),
        "/default-storage"
    );
}

#[test]
fn an_abandoned_claude_storage_lock_is_recovered_without_waiting() {
    let f = Fixture::new(ProviderKind::Claude);
    let path = f.homes[0].join(".storage-write.lock");
    fs::create_dir(&path).unwrap();
    File::open(&path)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(30))
        .unwrap();
    let store = f.store(ProviderKind::Claude, &f.homes[0]);
    let lock = store.lock().unwrap().unwrap();
    assert!(path.exists());
    drop(lock);
    assert!(!path.exists());
}

#[test]
fn losing_a_claude_lock_never_removes_the_replacement_lock() {
    let f = Fixture::new(ProviderKind::Claude);
    let store = f.store(ProviderKind::Claude, &f.homes[0]);
    let lock = store.lock().unwrap().unwrap();
    let path = f.homes[0].join(".storage-write.lock");
    // Keep the original inode alive to prevent the filesystem reusing its number.
    fs::rename(&path, f.root.join("old-lock")).unwrap();
    fs::create_dir(&path).unwrap();
    if let StoreLock::Directory(lock) = &lock {
        assert!(lock.check().is_err());
    }
    drop(lock);
    assert!(path.exists());
}

#[cfg(target_os = "macos")]
#[test]
fn a_keychain_refresh_after_the_scan_wins_over_the_scanned_candidate() {
    use std::cell::Cell;
    struct Racing<'a> {
        inner: &'a FakeKeyring,
        service: String,
        reads: Cell<usize>,
    }
    impl Keyring for Racing<'_> {
        fn get(&self, service: &str, account: &str) -> io::Result<Option<Vec<u8>>> {
            if service == self.service {
                self.reads.set(self.reads.get() + 1);
                if self.reads.get() == 2 {
                    self.inner.set(
                        service,
                        account,
                        &json!({
                            "claudeAiOauth":{"accessToken":"provider-refreshed-concurrently"},
                            "mcpOAuth":{"jira|fixture":claude_token("newest",300)}
                        })
                        .to_string()
                        .into_bytes(),
                    )?;
                }
            }
            self.inner.get(service, account)
        }
        fn set(&self, service: &str, account: &str, bytes: &[u8]) -> io::Result<()> {
            self.inner.set(service, account, bytes)
        }
        fn insert(&self, service: &str, account: &str, bytes: &[u8]) -> io::Result<Vec<u8>> {
            self.inner.insert(service, account, bytes)
        }
    }
    let f = Fixture::new(ProviderKind::Claude);
    f.put(
        &f.store(ProviderKind::Claude, &f.main),
        &json!({"mcpOAuth":{"jira|fixture":claude_token("scanned",200)}}),
    );
    let own = f.store(ProviderKind::Claude, &f.homes[0]);
    f.put(
        &own,
        &json!({"mcpOAuth":{"jira|fixture":claude_token("old",100)}}),
    );
    let racing = Racing {
        inner: &f.keyring,
        service: own.keychain.as_ref().unwrap().0.clone(),
        reads: Cell::new(0),
    };
    sync_with(ProviderKind::Claude, &f.env, &f.homes, &racing).unwrap();
    let got = own.read(&f.keyring).unwrap();
    assert_eq!(got["mcpOAuth"]["jira|fixture"]["accessToken"], "newest");
    assert_eq!(
        got["claudeAiOauth"]["accessToken"],
        "provider-refreshed-concurrently"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn a_locked_keychain_does_not_replace_the_existing_file_fallback() {
    let f = Fixture::new(ProviderKind::Claude);
    let own = f.store(ProviderKind::Claude, &f.homes[0]);
    f.put(
        &own,
        &json!({"claudeAiOauth":{"accessToken":"private-provider"}}),
    );
    fs::write(own.file(), "{\"fixture\":\"keep\"}").unwrap();
    f.keyring
        .denied
        .replace(Some(own.keychain.as_ref().unwrap().0.clone()));
    assert!(f.sync(ProviderKind::Claude).is_err());
    assert_eq!(
        fs::read_to_string(own.file()).unwrap(),
        "{\"fixture\":\"keep\"}"
    );
}

#[test]
fn sign_out_removes_only_the_resource_from_managed_claude_accounts() {
    let f = Fixture::new(ProviderKind::Claude);
    let main = f.store(ProviderKind::Claude, &f.main);
    let mut other = claude_token("other-resource", 100);
    other["serverUrl"] = json!("https://other.invalid/mcp");
    let initial = json!({"claudeAiOauth":{"accessToken":"provider-login"}, "mcpOAuth":{"jira|fixture":claude_token("mcp",100), "other|fixture":other}});
    f.put(&main, &initial);
    f.sync(ProviderKind::Claude).unwrap();
    forget_with(
        ProviderKind::Claude,
        &f.env,
        &f.homes,
        "https://fixture.invalid/mcp",
        &f.keyring,
    )
    .unwrap();
    assert_eq!(main.read(&f.keyring).unwrap(), initial);
    for home in &f.homes {
        let own = f
            .store(ProviderKind::Claude, home)
            .read(&f.keyring)
            .unwrap();
        assert!(own["mcpOAuth"].get("jira|fixture").is_none());
        assert_eq!(
            own["mcpOAuth"]["other|fixture"]["accessToken"],
            "other-resource"
        );
        assert!(own.get("claudeAiOauth").is_none());
    }
}

#[test]
fn sign_out_removes_only_the_resource_from_managed_codex_file_accounts() {
    let f = Fixture::new(ProviderKind::Codex);
    let main = f.store(ProviderKind::Codex, &f.main);
    let initial = json!({"jira|fixture":codex_token("mcp",100), "unrelated":{"kept":true}});
    f.put(&main, &initial);
    f.sync(ProviderKind::Codex).unwrap();
    for home in &f.homes {
        fs::write(home.join("auth.json"), "provider-login").unwrap();
    }
    forget_with(
        ProviderKind::Codex,
        &f.env,
        &f.homes,
        "https://fixture.invalid/mcp",
        &f.keyring,
    )
    .unwrap();
    assert_eq!(main.read(&f.keyring).unwrap(), initial);
    for home in &f.homes {
        assert!(
            f.store(ProviderKind::Codex, home)
                .read(&f.keyring)
                .unwrap()
                .get("jira|fixture")
                .is_none()
        );
        assert_eq!(
            fs::read_to_string(home.join("auth.json")).unwrap(),
            "provider-login"
        );
    }
}

#[test]
fn a_secure_storage_override_cannot_redirect_managed_writes_into_the_default_store() {
    let f = Fixture::new(ProviderKind::Claude);
    let store = f.store(ProviderKind::Claude, &f.main);
    let original = json!({"claudeAiOauth":{"accessToken":"keep"}, "mcpOAuth":{"jira|fixture":claude_token("mcp",100)}});
    f.put(&store, &original);
    let mut vars = f.env.vars();
    vars.push((
        "CLAUDE_SECURESTORAGE_CONFIG_DIR".into(),
        f.main.as_os_str().to_owned(),
    ));
    let env = f.env.with_vars(vars);
    let result = sync_with(ProviderKind::Claude, &env, &f.homes, &f.keyring);
    // When account environments strip this override, copying is safe. Otherwise fail
    // closed. In both cases the user's default store is byte-for-byte untouched.
    if env
        .for_account(ProviderKind::Claude, &f.homes[0])
        .var("CLAUDE_SECURESTORAGE_CONFIG_DIR")
        .is_some()
    {
        assert!(result.is_err());
    } else {
        result.unwrap();
    }
    assert_eq!(store.read(&f.keyring).unwrap(), original);
}
