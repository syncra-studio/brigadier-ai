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
        let binary = self.root.join("claude-checked");
        fs::write(&binary, b"binary .storage-write binary").unwrap();
        let cli = InstalledCli {
            version: CLAUDE_LOCK_SINCE,
            binary: &binary,
        };
        sync_with(kind, &self.env, &self.homes, Some(cli), &self.keyring)
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
    let binary = f.root.join("claude-checked");
    fs::write(&binary, CLAUDE_LOCK_NAME).unwrap();
    let cli = InstalledCli {
        version: CLAUDE_LOCK_SINCE,
        binary: &binary,
    };
    sync_with(ProviderKind::Claude, &f.env, &f.homes, Some(cli), &racing).unwrap();
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
    let binary = f.root.join("claude-checked");
    fs::write(&binary, CLAUDE_LOCK_NAME).unwrap();
    let cli = InstalledCli {
        version: CLAUDE_LOCK_SINCE,
        binary: &binary,
    };
    let result = sync_with(ProviderKind::Claude, &env, &f.homes, Some(cli), &f.keyring);
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

#[test]
fn claudes_credentials_are_rewritten_only_for_a_checked_claude() {
    let f = Fixture::new(ProviderKind::Claude);
    f.put(
        &f.store(ProviderKind::Claude, &f.main),
        &json!({"claudeAiOauth":{"accessToken":"login"},"mcpOAuth":{"jira|fixture":claude_token("mcp",100)}}),
    );
    f.keyring.writes.borrow_mut().clear();
    let names_it = f.root.join("names-it");
    fs::write(&names_it, b"binary .storage-write binary").unwrap();
    let silent = f.root.join("silent");
    fs::write(&silent, b"binary without the lock").unwrap();
    let gone = f.root.join("gone");
    let cli = |version, binary| InstalledCli { version, binary };
    let sync = |cli| sync_with(ProviderKind::Claude, &f.env, &f.homes, cli, &f.keyring);
    let own = f.store(ProviderKind::Claude, &f.homes[0]);
    for unchecked in [
        None,
        Some(cli("2.1.310", names_it.as_path())),
        Some(cli("not a version", names_it.as_path())),
        Some(cli("2.1.318", silent.as_path())),
        Some(cli("2.1.318", gone.as_path())),
    ] {
        let error = sync(unchecked).unwrap_err();
        assert_eq!(blocked(&error), Some(&Blocked::UncheckedCli));
        assert!(f.keyring.writes.borrow().is_empty());
        assert!(!own.file().exists());
    }
    sync(Some(cli("2.1.318", names_it.as_path()))).unwrap();
    assert_eq!(
        own.read(&f.keyring).unwrap()["mcpOAuth"]["jira|fixture"]["accessToken"],
        "mcp"
    );
    // Only Claude's item holds a login beside its MCP sign-ins.
    sync_with(ProviderKind::Codex, &f.env, &f.homes, None, &f.keyring).unwrap();
}

#[test]
fn the_lock_name_is_found_across_two_pieces_of_a_big_binary() {
    let f = Fixture::new(ProviderKind::Claude);
    let binary = f.root.join("big");
    let mut bytes = vec![b'x'; (1 << 20) - 4];
    bytes.extend_from_slice(CLAUDE_LOCK_NAME);
    fs::write(&binary, &bytes).unwrap();
    assert!(names_lock(&binary).unwrap());
    fs::write(&binary, vec![b'x'; 3 << 20]).unwrap();
    assert!(!names_lock(&binary).unwrap());
}

#[cfg(target_os = "macos")]
#[test]
fn a_keychain_refusal_stops_the_pass_at_the_first_item() {
    use std::cell::Cell;
    struct Refusing(Cell<usize>);
    impl Keyring for Refusing {
        fn get(&self, _: &str, _: &str) -> io::Result<Option<Vec<u8>>> {
            self.0.set(self.0.get() + 1);
            Err(io::Error::other(Blocked::KeychainRefused))
        }
        fn set(&self, _: &str, _: &str, _: &[u8]) -> io::Result<()> {
            panic!("nothing is written after a refusal");
        }
        fn insert(&self, _: &str, _: &str, _: &[u8]) -> io::Result<Vec<u8>> {
            panic!("nothing is written after a refusal");
        }
    }
    let f = Fixture::new(ProviderKind::Claude);
    let binary = f.root.join("names-it");
    fs::write(&binary, CLAUDE_LOCK_NAME).unwrap();
    let cli = InstalledCli {
        version: CLAUDE_LOCK_SINCE,
        binary: &binary,
    };
    let refusing = Refusing(Cell::new(0));
    let error =
        sync_with(ProviderKind::Claude, &f.env, &f.homes, Some(cli), &refusing).unwrap_err();
    assert_eq!(blocked(&error), Some(&Blocked::KeychainRefused));
    // One prompt at most, not one per account.
    assert_eq!(refusing.0.get(), 1);
}

#[test]
fn a_line_that_fits_goes_over_stdin_and_a_longer_one_goes_to_argv() {
    let line = keychain::stdin_line("Claude Code-credentials-ab", "me", "7b7d", 200).unwrap();
    assert_eq!(
        line,
        "add-generic-password -U -a \"me\" -s \"Claude Code-credentials-ab\" -X \"7b7d\"\n"
    );
    assert_eq!(
        keychain::stdin_line("Claude Code-credentials-ab", "me", "7b7d", line.len()).as_ref(),
        Some(&line)
    );
    assert_eq!(
        keychain::stdin_line("Claude Code-credentials-ab", "me", "7b7d", line.len() - 1),
        None
    );
    assert_eq!(
        keychain::add_args("Claude Code-credentials-ab", "me", "7b7d"),
        [
            "add-generic-password",
            "-U",
            "-a",
            "me",
            "-s",
            "Claude Code-credentials-ab",
            "-X",
            "7b7d"
        ]
    );
    // The limit stays below the longest line seen stored whole.
    const { assert!(keychain::STDIN_LINE_LIMIT < 2099) };
    assert_eq!(keychain::to_hex("{é".as_bytes()), "7bc3a9");
}

#[test]
fn a_name_that_would_need_quoting_never_goes_over_stdin() {
    for name in ["a\"b", "a\\b", "a\nb"] {
        assert_eq!(keychain::stdin_line("Claude Code", name, "7b", 200), None);
        assert_eq!(keychain::stdin_line(name, "me", "7b", 200), None);
    }
}

#[test]
fn a_password_printed_as_hex_reads_as_its_bytes() {
    assert_eq!(keychain::decode_password(b"{\"a\":1}\n"), b"{\"a\":1}");
    assert_eq!(
        keychain::decode_password(b"7b2261223a22c3a9227d\n"),
        "{\"a\":\"é\"}".as_bytes()
    );
    assert_eq!(keychain::decode_password(b"\n"), b"");
    assert_eq!(keychain::decode_password(b"abc\n"), b"abc");
}

#[test]
fn only_a_claude_item_listing_partitions_without_apple_tool_is_broken() {
    let item = |service: &str, account: &str, partitions: Option<&str>| {
        let partitions = partitions.map_or_else(String::new, |ids| {
            format!(
                "    entry 3:\n        authorizations (1): partition_id\n        \
                 don't-require-password\n        description: {ids}\n        applications: <null>\n"
            )
        });
        format!(
            "keychain: \"/login.keychain-db\"\nversion: 512\nclass: \"genp\"\nattributes:\n    \
             \"acct\"<blob>=\"{account}\"\n    \"svce\"<blob>=\"{service}\"\naccess: 5 entries\n    \
             entry 0:\n        authorizations (6): decrypt derive\n        \
             description: {service}\n{partitions}    entry 4:\n        \
             authorizations (1): change_acl\n        description: {service}\n"
        )
    };
    let dump = [
        item(
            "Claude Code-credentials-aa",
            "me",
            Some("teamid:7JQSPMWT79"),
        ),
        item("Claude Code-credentials-bb", "me", Some("apple-tool:")),
        item(
            "Claude Code-credentials-cc",
            "me",
            Some("teamid:7JQSPMWT79, apple-tool:"),
        ),
        item("Claude Code-credentials-dd", "me", None),
        item("codex", "me", Some("teamid:7JQSPMWT79")),
        item("Claude Code-credentials-ee", "you", Some("cdhash:12")),
    ]
    .concat();
    assert_eq!(
        keychain::broken_items(&dump)
            .into_iter()
            .collect::<Vec<_>>(),
        [
            ("Claude Code-credentials-aa".to_owned(), "me".to_owned()),
            ("Claude Code-credentials-ee".to_owned(), "you".to_owned()),
        ]
    );
}

#[test]
fn security_exits_are_told_apart() {
    assert!(keychain::refused(Some(128)));
    assert!(keychain::refused(Some(51)));
    assert!(!keychain::refused(Some(keychain::EXIT_NOT_FOUND)));
    assert!(!keychain::refused(None));
    assert!(keychain::is_claude("Claude Code-custom-oauth-credentials"));
    assert!(!keychain::is_claude("codex"));
    assert!(!keychain::is_claude(&keychain::backup_service(
        "Claude Code-credentials-aa"
    )));
}

#[cfg(target_os = "macos")]
#[test]
fn repair_runs_under_each_free_managed_lock_and_never_on_the_main_item() {
    #[derive(Default)]
    struct Repairs {
        inner: FakeKeyring,
        repaired: RefCell<Vec<(String, bool)>>,
        locks: RefCell<Vec<PathBuf>>,
    }
    impl Keyring for Repairs {
        fn get(&self, service: &str, account: &str) -> io::Result<Option<Vec<u8>>> {
            self.inner.get(service, account)
        }
        fn set(&self, service: &str, account: &str, data: &[u8]) -> io::Result<()> {
            self.inner.set(service, account, data)
        }
        fn insert(&self, service: &str, account: &str, data: &[u8]) -> io::Result<Vec<u8>> {
            self.inner.insert(service, account, data)
        }
        fn repair(&self, service: &str, _account: &str) -> io::Result<()> {
            let locked = self.locks.borrow().iter().all(|path| path.exists());
            self.repaired
                .borrow_mut()
                .push((service.to_owned(), locked));
            Ok(())
        }
    }
    let f = Fixture::new(ProviderKind::Claude);
    let binary = f.root.join("names-it");
    fs::write(&binary, CLAUDE_LOCK_NAME).unwrap();
    let cli = InstalledCli {
        version: CLAUDE_LOCK_SINCE,
        binary: &binary,
    };
    let free = f.store(ProviderKind::Claude, &f.homes[0]);
    let busy = f.store(ProviderKind::Claude, &f.homes[1]);
    let main = f.store(ProviderKind::Claude, &f.main);
    // Another writer holds the second account's lock.
    fs::create_dir(busy.home.join(".storage-write.lock")).unwrap();
    let repairs = Repairs::default();
    repairs
        .locks
        .borrow_mut()
        .push(free.home.join(".storage-write.lock"));
    sync_with(ProviderKind::Claude, &f.env, &f.homes, Some(cli), &repairs).unwrap();
    let service = |store: &Store| store.keychain.as_ref().unwrap().0.clone();
    assert_eq!(*repairs.repaired.borrow(), [(service(&free), true)]);
    assert_ne!(service(&free), service(&main));
}
