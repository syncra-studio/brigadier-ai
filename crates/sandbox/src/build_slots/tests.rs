use super::*;
use std::collections::BTreeMap;

#[test]
fn ecosystem_defaults_and_rust_wrapper_chain() {
    let path = Path::new("/home/me/build-tools/brigadier-build-slot");
    let env: BTreeMap<_, _> = environment(path, |_| None).into_iter().collect();
    assert_eq!(env["RUSTC_WRAPPER"], path.to_str().unwrap());
    assert_eq!(env["CMAKE_C_COMPILER_LAUNCHER"], path.to_str().unwrap());
    assert_eq!(env["CMAKE_CXX_COMPILER_LAUNCHER"], path.to_str().unwrap());
    assert_eq!(env["CARGO_BUILD_JOBS"], count().to_string());
    assert_eq!(env["CMAKE_BUILD_PARALLEL_LEVEL"], count().to_string());
    assert_eq!(env["GOFLAGS"], format!("-toolexec={}", path.display()));
    assert_eq!(
        env["GRADLE_OPTS"],
        format!("-Dorg.gradle.workers.max={}", count())
    );
    assert!(!env.contains_key(RUSTC_CHAIN));

    let user = BTreeMap::from([
        ("RUSTC_WRAPPER", "sccache"),
        ("CMAKE_C_COMPILER_LAUNCHER", "ccache"),
        ("CMAKE_CXX_COMPILER_LAUNCHER", ""),
        ("CARGO_BUILD_JOBS", "7"),
        ("CMAKE_BUILD_PARALLEL_LEVEL", "8"),
        ("GOFLAGS", "-tags=mine"),
        ("GRADLE_OPTS", "-Xmx1g"),
    ]);
    let env: BTreeMap<_, _> = environment(path, |key| user.get(key).map(|s| s.to_string()))
        .into_iter()
        .collect();
    assert_eq!(env.len(), 3);
    assert_eq!(env[RUSTC_CHAIN], "sccache");
    assert_eq!(env["RUSTC_WRAPPER"], path.to_str().unwrap());
    let env = environment(path, |key| {
        (key == "RUSTC_WRAPPER").then(|| path.to_string_lossy().into_owned())
    });
    assert!(!env.iter().any(|(key, _)| key == RUSTC_CHAIN));

    let inherited = BTreeMap::from([
        (
            "RUSTC_WRAPPER",
            "/other-daemon/build-tools/brigadier-build-slot",
        ),
        (RUSTC_CHAIN, "sccache"),
    ]);
    let env: BTreeMap<_, _> = environment(path, |key| inherited.get(key).map(|s| s.to_string()))
        .into_iter()
        .collect();
    assert_eq!(env[RUSTC_CHAIN], "sccache");
    assert_eq!(env["RUSTC_WRAPPER"], path.to_str().unwrap());
}

#[test]
fn cargo_build_wrapper_environment_is_chained_and_empty_disables_wrapping() {
    let path = Path::new("/home/me/brigadier-build-slot");
    let env: BTreeMap<_, _> = environment(path, |key| {
        (key == "CARGO_BUILD_RUSTC_WRAPPER").then(|| "sccache".into())
    })
    .into_iter()
    .collect();
    assert_eq!(env[RUSTC_CHAIN], "sccache");
    for key in ["RUSTC_WRAPPER", "CARGO_BUILD_RUSTC_WRAPPER"] {
        let env: BTreeMap<_, _> = environment(path, |name| (name == key).then(String::new))
            .into_iter()
            .collect();
        assert!(!env.contains_key("RUSTC_WRAPPER"));
        assert!(!env.contains_key(RUSTC_CHAIN));
    }
}

#[test]
fn rust_discovery_arguments() {
    for args in [
        vec!["-vV"],
        vec!["--version"],
        vec!["--print", "cfg"],
        vec!["--print=cfg"],
        vec!["--crate-name", "___", "-"],
        vec!["--crate-name=___"],
    ] {
        assert!(probe(
            &args.into_iter().map(OsString::from).collect::<Vec<_>>()
        ));
    }
    assert!(!probe(&["--crate-name".into(), "real_crate".into()]));
}

#[test]
fn go_launcher_with_spaces_quotes_both_parses() {
    let env: BTreeMap<_, _> = environment(Path::new("/home/a b/brigadier-build-slot"), |_| None)
        .into_iter()
        .collect();
    assert_eq!(
        env["GOFLAGS"],
        "\"-toolexec='/home/a b/brigadier-build-slot'\""
    );
}

#[test]
fn go_launcher_omits_flags_that_cannot_survive_go_quoting() {
    for path in [
        "/home/a'b c/brigadier-build-slot",
        "/home/a\"b c/brigadier-build-slot",
    ] {
        let env: BTreeMap<_, _> = environment(Path::new(path), |_| None).into_iter().collect();
        assert!(!env.contains_key("GOFLAGS"));
        assert!(env.contains_key("RUSTC_WRAPPER"));
    }
}

#[test]
fn preparing_preserves_existing_slot_and_its_lock() {
    let dir = std::env::temp_dir().join(format!("brigadier-slots-{}", std::process::id()));
    prepare_pool(&dir, 2).unwrap();
    let locked = acquire(&dir, 1).unwrap();
    prepare_pool(&dir, 2).unwrap();
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    options.write(true);
    let other = options.open(dir.join("slot-0")).unwrap();
    assert!(matches!(other.try_lock(), Err(TryLockError::WouldBlock)));
    // Other tests spawn processes concurrently: a fork can briefly hold a CLOEXEC handle
    // until its exec. This checks preparation's inode, not process-death release.
    locked.unlock().unwrap();
    drop(locked);
    other.try_lock().unwrap();
    drop(other);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn preparing_installs_the_multicall_launcher_and_keeps_live_slots() {
    let dir = std::env::temp_dir().join(format!("brigadier-launcher-{}", std::process::id()));
    let pool = dir.join("shared-slots");
    let data = dir.join("data");
    let cli = std::env::current_exe().unwrap();
    prepare_in(&pool, &data, &cli).unwrap();
    assert!(wrapper_path(&data).is_file());
    let held = acquire(&pool, count()).unwrap();
    prepare_in(&pool, &data, &cli).unwrap();
    let mut locks = Vec::new();
    for i in 0..count() {
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        options.write(true);
        let file = options.open(pool.join(format!("slot-{i}"))).unwrap();
        if file.try_lock().is_ok() {
            locks.push(file);
        }
    }
    assert_eq!(locks.len(), count() - 1);
    drop(held);
    drop(locks);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn daemons_keep_independent_launchers_and_a_shared_pool() {
    let dir = std::env::temp_dir().join(format!("brigadier-daemon-slots-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let pool = dir.join("shared-slots");
    let installed_cli = dir.join("installed-cli");
    let dev_cli = dir.join("dev-cli");
    fs::write(&installed_cli, "installed").unwrap();
    fs::write(&dev_cli, "dev").unwrap();
    let installed = dir.join("installed-data");
    let dev = dir.join("dev-data");
    prepare_in(&pool, &installed, &installed_cli).unwrap();
    let held = acquire(&pool, 1).unwrap();
    prepare_in(&pool, &dev, &dev_cli).unwrap();
    assert_eq!(fs::read(wrapper_path(&installed)).unwrap(), b"installed");
    assert_eq!(fs::read(wrapper_path(&dev)).unwrap(), b"dev");
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    options.write(true);
    let file = options.open(pool.join("slot-0")).unwrap();
    assert!(matches!(file.try_lock(), Err(TryLockError::WouldBlock)));
    fs::remove_file(dev_cli).unwrap();
    assert!(wrapper_path(&installed).is_file());
    #[cfg(unix)]
    assert!(!wrapper_path(&dev).is_file());
    held.unlock().unwrap();
    drop(held);
    drop(file);
    fs::remove_dir_all(dir).unwrap();
}

struct ConfigDir(PathBuf);

impl ConfigDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "brigadier-build-config-{name}-{}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn write(&self, path: &str, text: &str) -> PathBuf {
        let path = self.0.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        path
    }
}

impl Drop for ConfigDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn cargo_config_precedence_relative_paths_and_explicit_disable() {
    let dir = ConfigDir::new("cargo");
    dir.write(
        "cargo-home/config.toml",
        "[build]\nrustc-wrapper = 'global-cache'\n",
    );
    dir.write(
        "repo/.cargo/config.toml",
        "[ build ]\nrustc-wrapper = './bin/cache'\nrustc = './bin/custom-rustc'\n",
    );
    dir.write(
        "repo/nested/.cargo/config.toml",
        "build = { rustc-workspace-wrapper = './workspace-cache' }\n",
    );
    let cwd = dir.0.join("repo/nested").canonicalize().unwrap();
    let cargo_home = dir.0.join("cargo-home");
    let vars = BTreeMap::from([("CARGO_HOME", cargo_home.to_string_lossy().into_owned())]);
    let launcher = dir.0.join("brigadier-build-slot");
    let env: BTreeMap<_, _> = environment_in(&launcher, Some(&cwd), |key| vars.get(key).cloned())
        .into_iter()
        .collect();
    let repo = dir.0.join("repo").canonicalize().unwrap();
    assert_eq!(env[RUSTC_CHAIN], repo.join("./bin/cache").to_str().unwrap());
    assert_eq!(
        env[RUSTC_TOOL],
        repo.join("./bin/custom-rustc").to_str().unwrap()
    );
    assert_eq!(
        env[RUSTC_WORKSPACE],
        cwd.join("./workspace-cache").to_str().unwrap()
    );
    assert!(env.keys().all(|key| INJECTED_ENV.contains(&key.as_str())));

    dir.write(
        "repo/nested/.cargo/config",
        "[build]\nrustc-wrapper = 'nearest-cache'\n",
    );
    let env: BTreeMap<_, _> = environment_in(&launcher, Some(&cwd), |key| vars.get(key).cloned())
        .into_iter()
        .collect();
    assert_eq!(
        env[RUSTC_CHAIN], "nearest-cache",
        "legacy config wins over config.toml"
    );
    let env: BTreeMap<_, _> = environment_in(&launcher, Some(&cwd), |key| {
        if key == "RUSTC_WRAPPER" {
            Some("environment-cache".into())
        } else {
            vars.get(key).cloned()
        }
    })
    .into_iter()
    .collect();
    assert_eq!(env[RUSTC_CHAIN], "environment-cache");

    dir.write("repo/nested/.cargo/config", "[build]\nrustc-wrapper = ''\n");
    let env: BTreeMap<_, _> = environment_in(&launcher, Some(&cwd), |key| vars.get(key).cloned())
        .into_iter()
        .collect();
    assert!(!env.contains_key("RUSTC_WRAPPER"));
    assert!(!env.contains_key(RUSTC_CHAIN));
    let env: BTreeMap<_, _> = environment_in(&launcher, None, |key| vars.get(key).cloned())
        .into_iter()
        .collect();
    assert_eq!(env[RUSTC_CHAIN], "global-cache");
}

#[test]
fn persistent_go_flags_are_preserved_before_toolexec() {
    let dir = ConfigDir::new("goenv");
    let file = dir.write(
        "custom-goenv",
        "# go env -w output\nGOOS=linux\nGOFLAGS=-tags=mine -trimpath\n",
    );
    let vars = BTreeMap::from([("GOENV", file.to_string_lossy().into_owned())]);
    let launcher = Path::new("/tools/brigadier-build-slot");
    let env: BTreeMap<_, _> = environment_in(launcher, None, |key| vars.get(key).cloned())
        .into_iter()
        .collect();
    assert_eq!(
        env["GOFLAGS"],
        "-tags=mine -trimpath -toolexec=/tools/brigadier-build-slot"
    );
    let env: BTreeMap<_, _> = environment_in(launcher, None, |key| {
        if key == "GOFLAGS" {
            Some("-race".into())
        } else {
            vars.get(key).cloned()
        }
    })
    .into_iter()
    .collect();
    assert!(
        !env.contains_key("GOFLAGS"),
        "explicit environment flags remain untouched"
    );
    let env: BTreeMap<_, _> =
        environment_in(launcher, None, |key| (key == "GOENV").then(|| "off".into()))
            .into_iter()
            .collect();
    assert_eq!(env["GOFLAGS"], "-toolexec=/tools/brigadier-build-slot");
    fs::write(&file, "GOFLAGS=-tags=mine -toolexec=user-cache\n").unwrap();
    let env: BTreeMap<_, _> = environment_in(launcher, None, |key| vars.get(key).cloned())
        .into_iter()
        .collect();
    assert!(
        !env.contains_key("GOFLAGS"),
        "do not replace a user's persistent tool launcher"
    );
}

#[test]
fn go_flags_default_to_the_user_config_directory() {
    let dir = ConfigDir::new("go-default");
    let vars = BTreeMap::from([
        ("HOME", dir.0.to_string_lossy().into_owned()),
        (
            "APPDATA",
            dir.0.join("roaming").to_string_lossy().into_owned(),
        ),
        (
            "XDG_CONFIG_HOME",
            dir.0.join("xdg").to_string_lossy().into_owned(),
        ),
    ]);
    let path = if cfg!(target_os = "macos") {
        dir.0.join("Library/Application Support/go/env")
    } else if cfg!(windows) {
        dir.0.join("roaming/go/env")
    } else {
        dir.0.join("xdg/go/env")
    };
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, "GOFLAGS=-tags=default_user\n").unwrap();
    let env: BTreeMap<_, _> =
        environment_in(Path::new("/tools/brigadier-build-slot"), None, |key| {
            vars.get(key).cloned()
        })
        .into_iter()
        .collect();
    assert_eq!(
        env["GOFLAGS"],
        "-tags=default_user -toolexec=/tools/brigadier-build-slot"
    );
}
