//! Windows project links without Developer Mode. A manifest distinguishes our fallback
//! copies from the user's files; a copy edited since launch is no longer ours to replace.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::os::windows::fs::{FileTypeExt, symlink_dir, symlink_file};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{MCP_COPY, SETTINGS_COPIES, UNLINKED};

const MANIFEST: &str = ".claude/.project-links.json";

#[derive(Deserialize, Serialize)]
enum Placed {
    Link(PathBuf),
    Copy(String),
}

impl Placed {
    fn unchanged(&self, path: &Path) -> bool {
        match self {
            Self::Link(target) => fs::read_link(path).is_ok_and(|found| found == *target),
            Self::Copy(hash) => {
                fs::symlink_metadata(path).is_ok_and(|meta| meta.is_file())
                    && digest(path).is_ok_and(|found| found == *hash)
            }
        }
    }
}

fn digest(path: &Path) -> io::Result<String> {
    Ok(Sha256::digest(fs::read(path)?)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// No recursive deletion: a junction is removed as a directory entry, never its contents.
fn remove(path: &Path) -> io::Result<()> {
    let kind = fs::symlink_metadata(path)?.file_type();
    if kind.is_symlink_dir() || kind.is_dir() {
        fs::remove_dir(path)
    } else {
        fs::remove_file(path)
    }
}

/// Try the usual link first. Junctions require no symlink privilege; file copies also work
/// across volumes and never let a CLI write through to the project's settings.
fn place(target: &Path, link: &Path, try_symlink: bool) -> io::Result<Placed> {
    let dir = target.is_dir();
    if try_symlink {
        let made = if dir {
            symlink_dir(target, link)
        } else {
            symlink_file(target, link)
        };
        if made.is_ok() {
            return Ok(Placed::Link(fs::read_link(link)?));
        }
    }
    if dir {
        junction::create(target, link)?;
        Ok(Placed::Link(fs::read_link(link)?))
    } else {
        // Exclusive creation refuses an unexpected file or link at the destination.
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(link)?;
        io::copy(&mut fs::File::open(target)?, &mut output)?;
        Ok(Placed::Copy(digest(link)?))
    }
}

fn allowed(name: &str) -> bool {
    if name == ".mcp.json" {
        return true;
    }
    let Some(name) = name.strip_prefix(".claude/") else {
        return false;
    };
    !name.is_empty()
        && !name.contains(['/', '\\', ':'])
        && name != "."
        && name != ".."
        && name != ".project-links.json"
        && !UNLINKED.contains(&name)
}

pub(super) fn sync_links(cwd: &Path, wanted: &[(PathBuf, PathBuf)]) -> io::Result<()> {
    sync(cwd, wanted, true)
}

fn sync(cwd: &Path, wanted: &[(PathBuf, PathBuf)], try_symlink: bool) -> io::Result<()> {
    let manifest = cwd.join(MANIFEST);
    let mut placed: BTreeMap<String, Placed> = match fs::read(&manifest) {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(err) if err.kind() == io::ErrorKind::NotFound => BTreeMap::new(),
        Err(err) => return Err(err),
    };
    // Validate the entire manifest before acting on any of it.
    if placed.keys().any(|name| !allowed(name)) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid project link name",
        ));
    }
    let mut retained = BTreeMap::new();
    for (name, old) in placed {
        let link = cwd.join(&name);
        if !old.unchanged(&link) {
            continue;
        }
        if wanted.iter().any(|(path, _)| *path == link) {
            retained.insert(name, old);
        } else {
            remove(&link)?;
        }
    }
    placed = retained;
    for (link, target) in wanted {
        let name = link
            .strip_prefix(cwd)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))?
            .to_string_lossy()
            .replace('\\', "/");
        if !allowed(&name) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid project link name",
            ));
        }
        if fs::symlink_metadata(link).is_ok() {
            let Some(old) = placed.get(&name) else {
                continue;
            };
            let same = match old {
                Placed::Link(_) => link
                    .canonicalize()
                    .ok()
                    .zip(target.canonicalize().ok())
                    .is_some_and(|(left, right)| left == right),
                Placed::Copy(hash) => digest(target).is_ok_and(|found| found == *hash),
            };
            if same {
                continue;
            }
        }
        if let Some(parent) = link.parent() {
            fs::create_dir_all(parent)?;
        }
        // Build beside the destination, so file replacement is one rename on the same
        // volume. CLI readers see a complete old or new file, including fallback copies.
        let temp = link.with_file_name(format!(".project-link-{}", uuid::Uuid::new_v4()));
        let publish = || {
            let entry = place(target, &temp, try_symlink)?;
            if fs::symlink_metadata(link)
                .is_ok_and(|meta| meta.file_type().is_symlink_dir() || meta.is_dir())
            {
                // Windows cannot rename over an existing directory junction. Stage its
                // replacement first, and remove only the directory entry, never its tree.
                remove(link)?;
            }
            fs::rename(&temp, link)?;
            Ok(entry)
        };
        let result: io::Result<Placed> = publish();
        if result.is_err() {
            let _ = remove(&temp);
        }
        placed.insert(name, result?);
        // Record every successful placement, so a later failure can be retried safely.
        super::write_copy(cwd, MANIFEST, &serde_json::to_string(&placed)?)?;
    }
    if placed.is_empty() {
        match fs::remove_file(&manifest) {
            Err(err) if err.kind() != io::ErrorKind::NotFound => return Err(err),
            _ => {}
        }
    } else {
        super::write_copy(cwd, MANIFEST, &serde_json::to_string(&placed)?)?;
    }
    for name in std::iter::once(MCP_COPY).chain(SETTINGS_COPIES.iter().map(|(_, copy)| *copy)) {
        let copy = cwd.join(name);
        if !wanted.iter().any(|(_, target)| *target == copy) {
            match fs::remove_file(copy) {
                Err(err) if err.kind() != io::ErrorKind::NotFound => return Err(err),
                _ => {}
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod windows_tests {
    use super::*;
    use crate::claude::files::{link_project, unlink_project};

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "20261020-1537-issues-sweep-w16-{}",
                uuid::Uuid::new_v4()
            ));
            fs::create_dir_all(&root).unwrap();
            Self(root)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn project_setup_is_available_and_unlinked_without_touching_sources() {
        let root = Temp::new();
        let (project, cwd) = (root.0.join("project"), root.0.join("thread"));
        fs::create_dir_all(&cwd).unwrap();
        for name in ["hooks", "skills", "agents", "commands"] {
            fs::create_dir_all(project.join(".claude").join(name)).unwrap();
            fs::write(project.join(".claude").join(name).join("item"), name).unwrap();
        }
        fs::write(project.join(".mcp.json"), "{}").unwrap();
        fs::write(project.join(".claude/settings.json"), "{}").unwrap();
        fs::write(project.join(".claude/settings.local.json"), "{}").unwrap();
        link_project(&cwd, &project).unwrap();
        assert_eq!(fs::read_to_string(cwd.join(".mcp.json")).unwrap(), "{}");
        for name in ["hooks", "skills", "agents", "commands"] {
            assert_eq!(
                fs::read_to_string(cwd.join(".claude").join(name).join("item")).unwrap(),
                name
            );
        }
        for name in ["settings.json", "settings.local.json"] {
            assert!(cwd.join(".claude").join(name).is_file());
        }
        unlink_project(&cwd).unwrap();
        assert!(!cwd.join(".mcp.json").exists());
        assert!(!cwd.join(".claude/skills").exists());
        assert!(project.join(".claude/skills/item").is_file());
    }

    #[test]
    fn fallback_copies_refresh_and_junction_cleanup_preserves_the_project() {
        let root = Temp::new();
        let (project, cwd) = (root.0.join("project"), root.0.join("workers"));
        fs::create_dir_all(project.join("skills")).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        let source = project.join("settings.json");
        let link = cwd.join(".claude/settings.json");
        fs::write(&source, "first").unwrap();
        fs::write(project.join("skills/item"), "keep").unwrap();
        let wanted = [
            (link.clone(), source.clone()),
            (cwd.join(".claude/skills"), project.join("skills")),
        ];
        sync(&cwd, &wanted, false).unwrap();
        assert!(!link.is_symlink());
        assert!(junction::exists(cwd.join(".claude/skills")).unwrap());
        fs::write(&source, "second").unwrap();
        sync(&cwd, &wanted, false).unwrap();
        assert_eq!(fs::read_to_string(&link).unwrap(), "second");
        fs::write(&link, "user edit").unwrap();
        sync(&cwd, &[], false).unwrap();
        assert_eq!(fs::read_to_string(&link).unwrap(), "user edit");
        assert_eq!(fs::read_to_string(&source).unwrap(), "second");
        assert_eq!(
            fs::read_to_string(project.join("skills/item")).unwrap(),
            "keep"
        );
        assert!(!cwd.join(".claude/skills").exists());
    }

    #[test]
    fn file_copies_are_published_complete_for_concurrent_readers() {
        let root = Temp::new();
        let (project, cwd) = (root.0.join("project"), root.0.join("workers"));
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        let source = project.join("settings.json");
        let link = cwd.join(".claude/settings.json");
        let wanted = [(link.clone(), source.clone())];
        let text = serde_json::json!({ "padding": "x".repeat(65536), "version": 0 });
        fs::write(&source, text.to_string()).unwrap();
        sync(&cwd, &wanted, false).unwrap();
        let ready = std::sync::Arc::new(std::sync::Barrier::new(2));
        let reader_ready = ready.clone();
        let reader = std::thread::spawn(move || {
            reader_ready.wait();
            for _ in 0..1000 {
                let bytes = fs::read(&link).expect("the live settings file never disappears");
                let value: serde_json::Value = serde_json::from_slice(&bytes)
                    .expect("the live settings file is complete JSON");
                assert_eq!(value["padding"].as_str().unwrap().len(), 65536);
            }
        });
        ready.wait();
        for version in 1..30 {
            let text = serde_json::json!({ "padding": "x".repeat(65536), "version": version });
            fs::write(&source, text.to_string()).unwrap();
            sync(&cwd, &wanted, false).unwrap();
        }
        reader.join().unwrap();
    }

    #[test]
    fn native_mcp_paths_and_powershell_hooks_run_in_the_project() {
        let root = Temp::new();
        let (project, cwd) = (root.0.join("project space"), root.0.join("thread"));
        fs::create_dir_all(project.join(".claude")).unwrap();
        fs::create_dir_all(project.join("scripts")).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        fs::write(project.join("scripts/server.js"), "").unwrap();
        fs::write(project.join("scripts/server.ps1"), "").unwrap();
        let mcp = serde_json::json!({ "mcpServers": {
            "script": { "command": "node.exe", "args": ["scripts\\server.js"] },
            "own": { "command": ".\\scripts\\server.js" },
            "powershell": { "command": "PowerShell.EXE", "args": ["-File", "scripts\\server.ps1"] },
            "pwsh": { "command": "pwsh", "args": ["-File", "scripts/server.ps1"] }
        }});
        fs::write(project.join(".mcp.json"), mcp.to_string()).unwrap();
        let settings = serde_json::json!({ "hooks": { "SessionStart": [{ "hooks": [{
            "type": "command", "shell": "powershell",
            "command": "$env:CLAUDE_PROJECT_DIR | Set-Content -LiteralPath where.txt"
        }] }] } });
        fs::write(project.join(".claude/settings.json"), settings.to_string()).unwrap();
        link_project(&cwd, &project).unwrap();
        let mcp: serde_json::Value =
            serde_json::from_slice(&fs::read(cwd.join(".mcp.json")).unwrap()).unwrap();
        let script = project.join("scripts/server.js");
        assert_eq!(
            Path::new(mcp["mcpServers"]["script"]["args"][0].as_str().unwrap()),
            script
        );
        assert_eq!(
            Path::new(mcp["mcpServers"]["own"]["command"].as_str().unwrap())
                .canonicalize()
                .unwrap(),
            script.canonicalize().unwrap()
        );
        for runner in ["powershell", "pwsh"] {
            assert_eq!(
                Path::new(mcp["mcpServers"][runner]["args"][1].as_str().unwrap()),
                project.join("scripts/server.ps1")
            );
        }
        let settings: serde_json::Value =
            serde_json::from_slice(&fs::read(cwd.join(".claude/settings.json")).unwrap()).unwrap();
        let command = settings["hooks"]["SessionStart"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        assert!(
            std::process::Command::new("powershell.exe")
                .args(["-NoProfile", "-NonInteractive", "-Command", command])
                .env(super::super::PROJECT_DIR_ENV, &project)
                .current_dir(&cwd)
                .status()
                .unwrap()
                .success()
        );
        let found = fs::read_to_string(project.join("where.txt")).unwrap();
        assert_eq!(
            Path::new(found.trim()).canonicalize().unwrap(),
            project.canonicalize().unwrap()
        );
        // A new settings file is picked up on the next launch, including a transformed copy.
        fs::write(project.join(".claude/settings.json"), "{}").unwrap();
        link_project(&cwd, &project).unwrap();
        assert_eq!(
            fs::read_to_string(cwd.join(".claude/settings.json")).unwrap(),
            "{}"
        );
        unlink_project(&cwd).unwrap();
        assert!(!cwd.join(MCP_COPY).exists());
    }

    #[test]
    fn preexisting_files_and_folders_are_not_taken_over() {
        let root = Temp::new();
        let (project, cwd) = (root.0.join("project"), root.0.join("thread"));
        for dir in [&project, &cwd] {
            fs::create_dir_all(dir.join(".claude/skills")).unwrap();
        }
        for dir in [&project, &cwd] {
            fs::write(
                dir.join(".claude/settings.json"),
                dir.to_string_lossy().as_bytes(),
            )
            .unwrap();
        }
        let before = fs::read(cwd.join(".claude/settings.json")).unwrap();
        link_project(&cwd, &project).unwrap();
        unlink_project(&cwd).unwrap();
        assert_eq!(fs::read(cwd.join(".claude/settings.json")).unwrap(), before);
        assert!(cwd.join(".claude/skills").is_dir());
    }
}
