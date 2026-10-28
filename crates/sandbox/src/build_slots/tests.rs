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
    assert_eq!(env.len(), 2);
    assert_eq!(env[RUSTC_CHAIN], "sccache");
    assert_eq!(env["RUSTC_WRAPPER"], path.to_str().unwrap());
    let env = environment(path, |key| {
        (key == "RUSTC_WRAPPER").then(|| path.to_string_lossy().into_owned())
    });
    assert!(!env.iter().any(|(key, _)| key == RUSTC_CHAIN));
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
    let cli = std::env::current_exe().unwrap();
    prepare_in(&dir, &cli).unwrap();
    assert!(wrapper_in(&dir).is_file());
    let held = acquire(&dir.join("build-slots"), count()).unwrap();
    prepare_in(&dir, &cli).unwrap();
    let mut locks = Vec::new();
    for i in 0..count() {
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        options.write(true);
        let file = options
            .open(dir.join(format!("build-slots/slot-{i}")))
            .unwrap();
        if file.try_lock().is_ok() {
            locks.push(file);
        }
    }
    assert_eq!(locks.len(), count() - 1);
    drop(held);
    drop(locks);
    fs::remove_dir_all(dir).unwrap();
}
