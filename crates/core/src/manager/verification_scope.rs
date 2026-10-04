//! Conservative, candidate-local verification. Unknown inputs always retain full checks.
use std::{collections::BTreeSet, path::Path, process::Command};

use brigadier_git::{DiffStat, Oid, Repo, WorktreeSpec};
use serde::Deserialize;

use super::{SessionManager, blocking, gates::risky_path, git_error};
use crate::work::{Task, VerificationScope};

const SCOPED_FILES: usize = 5;
const SCOPED_LINES: u32 = 150;
const SHARED_FILES: &[&str] = &[
    "Cargo.toml",
    "Cargo.lock",
    "package.json",
    "pnpm-lock.yaml",
    "package-lock.json",
    "yarn.lock",
    "bun.lock",
    "bun.lockb",
    "pnpm-workspace.yaml",
    "build.rs",
    "Makefile",
    "CMakeLists.txt",
    "rust-toolchain",
    "rust-toolchain.toml",
    "tauri.conf.json",
    ".npmrc",
    ".pnpmfile.cjs",
    ".oxlintrc.json",
    "rustfmt.toml",
    "clippy.toml",
    "Justfile",
    "Jenkinsfile",
    ".gitlab-ci.yml",
];
const SHARED_DIRS: &[&str] = &[
    ".github",
    ".cargo",
    ".buildkite",
    "scripts",
    "registry",
    "ci",
];

fn full(reason: &str) -> VerificationScope {
    VerificationScope::Full {
        reason: reason.into(),
    }
}

fn eligible(stat: &DiffStat, paths: &[String]) -> Result<(), &'static str> {
    if paths.is_empty() || stat.files.is_empty() {
        return Err("Unknown or empty diff");
    }
    if paths.len() > SCOPED_FILES {
        return Err("More than five paths");
    }
    if stat.files.iter().any(|f| f.binary) {
        return Err("Binary change");
    }
    if stat
        .insertions
        .checked_add(stat.deletions)
        .is_none_or(|n| n > SCOPED_LINES)
    {
        return Err("More than 150 changed lines or unknown count");
    }
    for path in paths {
        if risky_path(path) {
            return Err("Risky path");
        }
        let name = path.rsplit('/').next().unwrap_or(path);
        if SHARED_FILES.contains(&name)
            || name.starts_with("tsconfig")
            || name.ends_with(".lock")
            || name.starts_with("build.")
            || name.contains(".config.")
            || name.ends_with(".conf.json")
            || path.split('/').any(|part| SHARED_DIRS.contains(&part))
        {
            return Err("Shared configuration or build input");
        }
    }
    Ok(())
}

#[derive(Deserialize)]
struct Metadata {
    packages: Vec<Package>,
    workspace_members: Vec<String>,
}
#[derive(Deserialize)]
struct Package {
    id: String,
    name: String,
    manifest_path: String,
    dependencies: Vec<Dependency>,
}
#[derive(Deserialize)]
struct Dependency {
    path: Option<String>,
}

fn determine(
    metadata: Metadata,
    root: &Path,
    paths: &[String],
    exported_types: bool,
) -> Option<VerificationScope> {
    let packages: Vec<_> = metadata
        .packages
        .iter()
        .filter(|p| metadata.workspace_members.contains(&p.id))
        .collect();
    // These scoped commands are Brigadier's contract, not a generic Rust recipe.
    // Unknown workspaces keep their README/CI-driven full verifier.
    if !["brigadier-desktop", "brigadier-ipc"]
        .iter()
        .all(|name| packages.iter().any(|p| p.name == *name))
    {
        return None;
    }
    let dirs: Vec<_> = packages
        .iter()
        .map(|p| Path::new(&p.manifest_path).parent())
        .collect::<Option<_>>()?;
    let mut affected = BTreeSet::new();
    let mut desktop = exported_types;
    for path in paths {
        desktop |= path.starts_with("apps/desktop/");
        let absolute = root.join(path);
        if let Some((index, _)) = dirs
            .iter()
            .enumerate()
            .filter(|(_, dir)| absolute.starts_with(dir))
            .max_by_key(|(_, dir)| dir.components().count())
        {
            // Resources and fixtures inside a crate can affect its build and tests too.
            affected.insert(index);
        } else if path.ends_with(".rs")
            || (!path.starts_with("apps/desktop/")
                && !super::gates::docs_only(&[crate::work::FileStat {
                    path: path.clone(),
                    insertions: 0,
                    deletions: 0,
                    binary: false,
                }]))
        {
            return None;
        }
    }
    loop {
        let before = affected.len();
        for (i, package) in packages.iter().enumerate() {
            if package.dependencies.iter().any(|dep| {
                dep.path
                    .as_ref()
                    .is_some_and(|path| affected.iter().any(|j| Path::new(path) == dirs[*j]))
            }) {
                affected.insert(i);
            }
        }
        if before == affected.len() {
            break;
        }
    }
    let mut crates: Vec<_> = affected
        .into_iter()
        .map(|i| packages[i].name.clone())
        .collect();
    if crates.iter().any(|name| {
        !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    }) {
        return None;
    }
    crates.sort();
    Some(VerificationScope::Scoped {
        reason: "Small change".into(),
        crates,
        desktop,
    })
}

impl SessionManager {
    pub(super) async fn verification_scope(&self, task: &Task) -> VerificationScope {
        let (Some(candidate), Some(workspace), Ok(repo)) =
            (&task.candidate, &task.workspace, self.task_repo(task))
        else {
            return full("Candidate scope unavailable");
        };
        let env = self.runtime.cli_env().clone();
        let (git, candidate, scratch) = (
            self.git.clone(),
            candidate.clone(),
            workspace.scratch.clone(),
        );
        blocking(move || {
            let repo = git.open(&repo).map_err(git_error)?;
            let Some(cargo) = env.which("cargo") else {
                return Ok(full("Cargo unavailable"));
            };
            let mut command = Command::new(cargo);
            command.envs(env.vars());
            candidate_scope(
                &repo,
                &Oid(candidate.onto),
                &Oid(candidate.commit),
                Path::new(&scratch),
                &mut command,
            )
        })
        .await
        .unwrap_or_else(|_| full("Candidate scope unavailable"))
    }
}

/// Inspect immutable Git objects, then run metadata in that exact candidate checkout.
fn candidate_scope(
    repo: &Repo,
    base: &Oid,
    commit: &Oid,
    scratch: &Path,
    cargo: &mut Command,
) -> crate::Result<VerificationScope> {
    let (stat, paths) = repo.diff_scope(base, commit).map_err(git_error)?;
    let docs = paths
        .iter()
        .map(|path| crate::work::FileStat {
            path: path.clone(),
            insertions: 0,
            deletions: 0,
            binary: false,
        })
        .collect::<Vec<_>>();
    if super::gates::docs_only(&docs) && !stat.files.iter().any(|f| f.binary) {
        return Ok(full("Documentation-only policy"));
    }
    if let Err(reason) = eligible(&stat, &paths) {
        return Ok(full(reason));
    }
    let exported_types = exported_types_changed(repo, base, commit, &paths).map_err(git_error)?;
    // Never read Cargo metadata from a worker checkout that may have moved or be dirty.
    let checkout = scratch.join(format!("verification-scope-{}", uuid::Uuid::new_v4()));
    repo.add_worktree(&checkout, WorktreeSpec::Detached { at: commit.clone() })
        .map_err(git_error)?;
    let scope = (|| {
        let output = cargo
            .args([
                "metadata",
                "--format-version",
                "1",
                "--no-deps",
                "--offline",
                "--locked",
            ])
            .current_dir(&checkout)
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let metadata = serde_json::from_slice(&output.stdout).ok()?;
        determine(
            metadata,
            &std::fs::canonicalize(&checkout).ok()?,
            &paths,
            exported_types,
        )
    })()
    .unwrap_or_else(|| full("Candidate metadata or affected checks unavailable"));
    if let Err(error) = repo.remove_worktree(&checkout, true) {
        tracing::warn!(%error, path = %checkout.display(), "scope worktree cleanup failed; removing scratch checkout");
        if checkout.exists() {
            std::fs::remove_dir_all(&checkout).map_err(|error| {
                crate::Error::Invalid(format!("scope scratch cleanup: {error}"))
            })?;
        }
        repo.prune_worktrees().map_err(git_error)?;
    }
    Ok(scope)
}

/// Scan whole changed files on both sides: removal of a derive/import must count too.
/// Deliberately retain comments and strings, so uncertain marker uses err toward checking.
fn ts_export_marker(source: &[u8]) -> bool {
    let Ok(source) = std::str::from_utf8(source) else {
        return true;
    };
    source
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .any(|word| {
            matches!(
                word,
                "TS" | "ts_rs" | "ts" | "TypeScript" | "export_all" | "export_to"
            )
        })
}

fn exported_types_changed(
    repo: &Repo,
    base: &Oid,
    commit: &Oid,
    paths: &[String],
) -> brigadier_git::Result<bool> {
    for path in paths.iter().filter(|path| path.ends_with(".rs")) {
        for revision in [base, commit] {
            if repo
                .file_at(revision, path)?
                .is_some_and(|source| ts_export_marker(&source))
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

pub(super) fn scoped_spec(spec: String, scope: &mut VerificationScope, sandboxed: bool) -> String {
    let VerificationScope::Scoped {
        crates, desktop, ..
    } = scope
    else {
        return spec;
    };
    let mut checks = String::from(
        "2. Scoped checks: small change. Run every command named in the task's done-when criteria and check every criterion as instructed above. Run these checks from the repository root unless a directory is specified:",
    );
    if crates.iter().any(|name| name == "brigadier-desktop") {
        checks.push_str("\n- First run `pnpm build`, then `pnpm --filter @brigadier/desktop stage-sidecar --debug`, before any Cargo build, test or clippy command (the desktop externalBin requires the staged sidecar).");
    }
    if !crates.is_empty() {
        let packages = crates
            .iter()
            .map(|name| format!(" -p {name}"))
            .collect::<String>();
        for command in ["build", "test", "clippy --all-targets"] {
            let flags = if command.starts_with("clippy") {
                " -- -D warnings"
            } else {
                ""
            };
            checks.push_str(&format!("\n- `cargo {command}{packages}{flags}`."));
        }
        checks.push_str("\n- `cargo fmt --check`.");
    }
    if *desktop {
        checks.push_str("\n- In `apps/desktop`: `pnpm typecheck`, `pnpm lint`, `pnpm test`, and `pnpm build`.\n- Regenerate bindings into scratch with `cargo run -p brigadier-ipc --bin gen-ts <scratch>/bindings`, then `diff -ru apps/desktop/src/ipc/generated <scratch>/bindings`; there must be no differences.");
    }
    checks.push_str(&format!("\n{}", super::gates::checks_setup(sandboxed)));
    // Replace only the general checklist; all evidence, failure and review rules remain.
    let Some(start) = spec.find("2. Run the project's checks") else {
        *scope = full("Verifier template could not be scoped");
        return spec;
    };
    let Some(end) = spec[start..].find("\n3. Check hygiene") else {
        *scope = full("Verifier template could not be scoped");
        return spec;
    };
    format!("{}{}{}", &spec[..start], checks, &spec[start + end..])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stat(lines: u32) -> DiffStat {
        DiffStat {
            insertions: lines,
            deletions: 0,
            files: vec![brigadier_git::FileStat {
                path: "crates/a/src/lib.rs".into(),
                insertions: lines,
                deletions: 0,
                binary: false,
            }],
        }
    }

    #[test]
    fn size_boundaries_and_unknown_counts() {
        let paths = (0..6).map(|i| format!("src/{i}.rs")).collect::<Vec<_>>();
        assert!(eligible(&stat(150), &paths[..5]).is_ok());
        assert!(eligible(&stat(150), &paths).is_err());
        assert!(eligible(&stat(151), &paths[..5]).is_err());
        let mut binary = stat(0);
        binary.files[0].binary = true;
        assert!(eligible(&binary, &paths[..1]).is_err());
        let mut overflow = stat(u32::MAX);
        overflow.deletions = 1;
        assert!(eligible(&overflow, &paths[..1]).is_err());
        assert!(eligible(&stat(0), &[]).is_err());
    }

    #[test]
    fn both_rename_endpoints_block_risky_and_shared_inputs() {
        for path in [
            "crates/git/src/lib.rs",
            "src/policy.rs",
            "Cargo.toml",
            "crates/a/Cargo.toml",
            "Cargo.lock",
            "apps/desktop/package.json",
            "pnpm-lock.yaml",
            "apps/desktop/tsconfig.json",
            "crates/a/build.rs",
            ".github/workflows/ci.yml",
            "registry/models.json",
        ] {
            for endpoints in [
                [path.to_owned(), "src/safe.rs".into()],
                ["src/safe.rs".into(), path.to_owned()],
            ] {
                assert!(eligible(&stat(0), &endpoints).is_err(), "{endpoints:?}");
            }
        }
    }

    fn metadata() -> Metadata {
        serde_json::from_value(serde_json::json!({
            "workspace_members": ["a", "b", "desktop", "ipc", "unrelated"],
            "packages": [
                {"id":"ipc", "name":"brigadier-ipc", "manifest_path":"/candidate/crates/ipc/Cargo.toml", "dependencies":[]},
                {"id":"a", "name":"a", "manifest_path":"/candidate/crates/a/Cargo.toml", "dependencies":[]},
                {"id":"b", "name":"b", "manifest_path":"/candidate/crates/b/Cargo.toml", "dependencies":[{"path":"/candidate/crates/a"}]},
                {"id":"desktop", "name":"brigadier-desktop", "manifest_path":"/candidate/apps/desktop/src-tauri/Cargo.toml", "dependencies":[{"path":"/candidate/crates/b"}]},
                {"id":"unrelated", "name":"unrelated", "manifest_path":"/candidate/crates/unrelated/Cargo.toml", "dependencies":[]}
            ]
        })).unwrap()
    }

    #[test]
    fn internal_edit_includes_dependents_and_stages_sidecar_before_cargo() {
        let scope = determine(
            metadata(),
            Path::new("/candidate"),
            &["crates/a/src/internal.rs".into()],
            false,
        )
        .unwrap();
        let VerificationScope::Scoped { crates, .. } = &scope else {
            panic!("not scoped")
        };
        assert_eq!(crates, &["a", "b", "brigadier-desktop"]);
        let spec = scoped_spec(
            "1. Criterion: `custom-check`\n2. Run the project's checks\n3. Check hygiene".into(),
            &mut scope.clone(),
            true,
        );
        assert!(spec.contains("custom-check"));
        assert!(spec.contains("every command named in the task's done-when criteria"));
        let build = spec.find("`pnpm build`").unwrap();
        let stage = spec.find("stage-sidecar --debug").unwrap();
        assert!(build < stage);
        for command in [
            "cargo build -p a -p b -p brigadier-desktop",
            "cargo test -p a",
            "cargo clippy --all-targets -p a -p b -p brigadier-desktop -- -D warnings",
        ] {
            assert!(stage < spec.find(command).unwrap());
        }
        assert!(spec.contains("cargo fmt --check"));
        for excluded in [
            "pnpm typecheck",
            "pnpm lint",
            "pnpm test",
            "--bin gen-ts",
            "diff -ru",
        ] {
            assert!(
                !spec.contains(excluded),
                "internal edit unnecessarily requested {excluded}"
            );
        }
        assert!(spec.contains("sandbox can't open windows"));
    }

    #[test]
    fn exported_type_and_desktop_edits_include_all_desktop_checks() {
        for path in ["crates/a/src/types.rs", "apps/desktop/src/app.tsx"] {
            let scope = determine(
                metadata(),
                Path::new("/candidate"),
                &[path.into()],
                path.ends_with(".rs"),
            )
            .unwrap();
            let spec = scoped_spec(
                "2. Run the project's checks\n3. Check hygiene".into(),
                &mut scope.clone(),
                false,
            );
            for check in [
                "pnpm typecheck",
                "pnpm lint",
                "pnpm test",
                "pnpm build",
                "--bin gen-ts",
                "diff -ru",
            ] {
                assert!(spec.contains(check), "{check}: {spec}");
            }
        }
    }

    #[test]
    fn crate_resources_include_cargo_checks_too() {
        for (path, expected) in [
            (
                "crates/a/fixtures/data.json",
                vec!["a", "b", "brigadier-desktop"],
            ),
            (
                "apps/desktop/src-tauri/capabilities/default.json",
                vec!["brigadier-desktop"],
            ),
        ] {
            let scope =
                determine(metadata(), Path::new("/candidate"), &[path.into()], false).unwrap();
            let VerificationScope::Scoped { crates, .. } = scope else {
                panic!("not scoped")
            };
            assert_eq!(crates, expected);
        }
    }

    #[test]
    fn unknown_scope_retains_the_full_spec() {
        assert!(
            determine(
                metadata(),
                Path::new("/candidate"),
                &["unknown/src/a.rs".into()],
                false
            )
            .is_none()
        );
        assert!(
            determine(
                metadata(),
                Path::new("/candidate"),
                &["other.py".into()],
                false
            )
            .is_none()
        );
        let spec = "2. Run the project's checks\n3. Check hygiene";
        assert_eq!(scoped_spec(spec.into(), &mut full("unknown"), false), spec);
    }
    #[test]
    fn ts_export_markers_err_toward_checking_consumers() {
        for source in [
            "#[derive(Serialize, TS)]",
            "use ts_rs::TS as Exported;",
            "#[ts(export)]",
            "impl TS for Wrapper {}",
            "Thing::export_all(config)",
            "Thing::export_to(path)",
            "// TypeScript export generated by a macro",
        ] {
            assert!(ts_export_marker(source.as_bytes()), "{source}");
        }
        assert!(ts_export_marker(&[0xff]));
        assert!(!ts_export_marker(b"fn internal() { let tests = true; }"));
    }

    #[test]
    fn candidate_inspection_recomputes_from_git_and_candidate_metadata() {
        let temp = std::env::temp_dir().join(format!("scope-test-{}", uuid::Uuid::new_v4()));
        let root = temp.join("repo");
        let scratch = temp.join("scratch");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&scratch).unwrap();
        let mut env: Vec<_> = std::env::vars_os()
            .filter(|(key, _)| !key.to_string_lossy().starts_with("GIT_"))
            .collect();
        for (key, value) in [
            ("GIT_CONFIG_NOSYSTEM", "1"),
            ("GIT_CONFIG_GLOBAL", "/dev/null"),
            ("GIT_AUTHOR_NAME", "Test"),
            ("GIT_AUTHOR_EMAIL", "test@example.com"),
            ("GIT_COMMITTER_NAME", "Test"),
            ("GIT_COMMITTER_EMAIL", "test@example.com"),
        ] {
            env.push((key.into(), value.into()));
        }
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .args(args)
                .current_dir(&root)
                .envs(env.clone())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        };
        git(&["init", "--quiet"]);
        std::fs::write(root.join("Cargo.toml"), "[workspace]\nresolver = \"2\"\nmembers = [\"crates/a\", \"crates/ipc\", \"apps/desktop/src-tauri\"]\n").unwrap();
        for (directory, name, dependency) in [
            ("crates/a", "a", ""),
            (
                "crates/ipc",
                "brigadier-ipc",
                "[dependencies]\na = { path = \"../a\" }\n",
            ),
            (
                "apps/desktop/src-tauri",
                "brigadier-desktop",
                "[dependencies]\na = { path = \"../../../crates/a\" }\n",
            ),
        ] {
            let directory = root.join(directory);
            std::fs::create_dir_all(directory.join("src")).unwrap();
            std::fs::write(directory.join("Cargo.toml"), format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n{dependency}")).unwrap();
            std::fs::write(directory.join("src/lib.rs"), "fn internal() {}\n").unwrap();
        }
        let output = Command::new("cargo")
            .args(["generate-lockfile", "--offline"])
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let commit = || {
            git(&["add", "."]);
            git(&["commit", "--quiet", "-m", "candidate"]);
            Oid(git(&["rev-parse", "HEAD"]))
        };
        let base = commit();
        let source = root.join("crates/a/src/lib.rs");
        std::fs::write(&source, "fn internal() { let value = 1; }\n").unwrap();
        let internal = commit();
        std::fs::write(
            &source,
            "#[derive(ts_rs::TS)]\npub struct Exported { pub value: u32 }\n",
        )
        .unwrap();
        let exported = commit();
        std::fs::write(&source, "fn internal() { let value = 2; }\n").unwrap();
        let removed = commit();
        std::fs::write(&source, "// large edit\n".repeat(151)).unwrap();
        let large = commit();
        // The live checkout is dirty and at a different commit throughout these inspections.
        std::fs::write(&source, "// TypeScript marker only in uncommitted work\n").unwrap();
        let engine = brigadier_git::Git::new("git".into(), env.clone());
        let repo = engine.open(&root).unwrap();
        let inspect = |from: &Oid, to: &Oid| {
            candidate_scope(&repo, from, to, &scratch, &mut Command::new("cargo")).unwrap()
        };
        for (from, to, consumers) in [
            (&base, &internal, false),
            (&base, &exported, true),
            (&exported, &removed, true),
            (&base, &removed, false),
        ] {
            let mut scope = inspect(from, to);
            let VerificationScope::Scoped {
                crates, desktop, ..
            } = &scope
            else {
                panic!("{scope:?}")
            };
            assert_eq!(*desktop, consumers);
            assert_eq!(crates, &["a", "brigadier-desktop", "brigadier-ipc"]);
            let spec = scoped_spec(
                "2. Run the project's checks\n3. Check hygiene".into(),
                &mut scope,
                false,
            );
            for command in [
                "pnpm typecheck",
                "pnpm lint",
                "pnpm test",
                "--bin gen-ts",
                "diff -ru",
            ] {
                assert_eq!(spec.contains(command), consumers, "{command}: {scope:?}");
            }
            assert!(spec.find("pnpm build").unwrap() < spec.find("stage-sidecar --debug").unwrap());
            assert!(
                spec.find("stage-sidecar --debug").unwrap() < spec.find("cargo build").unwrap()
            );
            assert_eq!(repo.worktrees().unwrap().len(), 1);
            assert_eq!(std::fs::read_dir(&scratch).unwrap().count(), 0);
        }
        assert!(matches!(
            inspect(&base, &large),
            VerificationScope::Full { .. }
        ));
        assert!(matches!(
            inspect(&large, &large),
            VerificationScope::Full { .. }
        ));
        std::fs::remove_dir_all(temp).unwrap();
    }
    #[test]
    fn other_rust_workspaces_fall_back_without_brigadier_commands() {
        for missing in ["brigadier-desktop", "brigadier-ipc"] {
            let mut data = metadata();
            data.packages.retain(|p| p.name != missing);
            assert!(
                determine(
                    data,
                    Path::new("/candidate"),
                    &["crates/a/src/lib.rs".into()],
                    false
                )
                .is_none()
            );
        }
        let mut data = metadata();
        data.packages.retain(|p| p.name == "a");
        assert!(
            determine(
                data,
                Path::new("/candidate"),
                &["crates/a/src/lib.rs".into()],
                false
            )
            .is_none()
        );
    }

    #[test]
    fn changed_prompt_markers_record_full_scope_instead_of_a_false_label() {
        for spec in [
            "New verifier template",
            "2. Run the project's checks but no hygiene marker",
        ] {
            let mut scope = VerificationScope::Scoped {
                reason: "Small change".into(),
                crates: vec!["a".into()],
                desktop: false,
            };
            assert_eq!(scoped_spec(spec.into(), &mut scope, false), spec);
            assert!(matches!(scope, VerificationScope::Full { .. }));
        }
    }
}
