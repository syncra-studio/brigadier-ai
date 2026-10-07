//! B13: workers honor the repository's instruction files whatever their vendor.
//!
//! What each CLI loads by itself (verified with claude 2.1.281 and codex 0.156.1):
//!
//! - **Claude Code**, with the adapter's `instructionFiles = claude-md-and-agents-md`: the root
//!   `CLAUDE.md`, `.claude/CLAUDE.md`, `.claude/rules/*.md` and the root `AGENTS.md` at start,
//!   and a subfolder's `CLAUDE.md` / `AGENTS.md` when it reads a file there. Nothing is added.
//! - **Codex**: only the `AGENTS.md` files from the repository root down to its working
//!   folder; no `CLAUDE.md`, and no nested files below. A Codex worker's prompt carries the
//!   `CLAUDE.md` files, `.claude/CLAUDE.md`, `.claude/rules/*.md`, and the nested
//!   `AGENTS.md` files below the root.
//!
//! Files whose content matches one the CLI loads anyway (a symlink, a copy, or a one-line
//! `@AGENTS.md` import) are not repeated.
//!
//! Precedence, stated in the prompt: the task spec wins over repository instructions, and a
//! file in a subfolder wins over one higher up for the files under that folder.
//!
//! The session's thread works from its own folder outside the repository, so it gets every
//! instruction file of its workspace in its prompt, the root `AGENTS.md` too, whatever its
//! vendor (THREAD-PLAN.md Q1).

use std::path::{Path, PathBuf};

use brigadier_providers::ProviderKind;

/// Bytes of instructions carried at most.
const MAX_BYTES: usize = 32 * 1024;

/// The instruction files `provider` does not load itself, formatted for the worker prompt.
pub(crate) async fn for_worker(provider: ProviderKind, worktree: &Path) -> String {
    if provider == ProviderKind::Claude {
        return String::new();
    }
    let worktree = worktree.to_owned();
    tokio::task::spawn_blocking(move || collect(&worktree))
        .await
        .unwrap_or_default()
}

/// Every instruction file of the workspace, formatted for the thread's prompt: its working
/// directory is its own folder outside the repository, so neither CLI finds them itself.
pub(crate) async fn for_thread(workspace: &Path) -> String {
    let workspace = workspace.to_owned();
    tokio::task::spawn_blocking(move || {
        collect_files(&workspace, true, "The user's request wins over them")
    })
    .await
    .unwrap_or_default()
}

fn collect(worktree: &Path) -> String {
    collect_files(worktree, false, "The task spec wins over them")
}

/// The instruction files in `worktree` (`root_agents`: its root `AGENTS.md` too), each once;
/// `precedence` says what wins over them.
fn collect_files(worktree: &Path, root_agents: bool, precedence: &str) -> String {
    let mut found = Vec::new();
    find(worktree, 0, &mut found);
    let rules = worktree.join(".claude").join("rules");
    if let Ok(entries) = std::fs::read_dir(&rules) {
        found.extend(
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "md")),
        );
    }
    let dot_claude = worktree.join(".claude").join("CLAUDE.md");
    if dot_claude.is_file() {
        found.push(dot_claude);
    }
    let root = worktree.join("AGENTS.md");
    // A worker's CLI loads the root AGENTS.md natively.
    found.retain(|path| path != &root);
    let root_text = std::fs::read_to_string(&root).unwrap_or_default();
    if root_agents && !root_text.trim().is_empty() {
        found.insert(0, root);
    }
    found.sort();
    found.dedup();
    // For a worker: what its CLI loads anyway.
    let native_root = if root_agents {
        String::new()
    } else {
        root_text
    };
    let mut text = String::new();
    for path in found {
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let content = content.trim();
        if content.is_empty() || imports_only(content) {
            continue;
        }
        let sibling = path.with_file_name("AGENTS.md");
        if sibling != path
            && std::fs::read_to_string(&sibling).is_ok_and(|other| other.trim() == content)
        {
            continue;
        }
        if !native_root.is_empty() && content == native_root.trim() {
            continue;
        }
        let relative = path.strip_prefix(worktree).unwrap_or(&path);
        let folder = relative
            .parent()
            .filter(|p| !p.as_os_str().is_empty() && !p.starts_with(".claude"));
        let scope = folder
            .map(|p| format!(" (applies to files under {}/)", p.display()))
            .unwrap_or_default();
        let block = format!("\n\n### {}{scope}\n{content}", relative.display());
        if text.len() + block.len() > MAX_BYTES {
            text.push_str("\n\n[more instruction files were left out for length; read them in the repository]");
            break;
        }
        text.push_str(&block);
    }
    if text.is_empty() {
        return text;
    }
    format!(
        "\n\nRepository instructions (follow them like your own AGENTS.md). {precedence}, and a file in a subfolder wins over one higher up for the files under it:{text}"
    )
}

/// A one-line import of `AGENTS.md`, which Codex loads anyway.
fn imports_only(content: &str) -> bool {
    content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .all(|line| line == "@AGENTS.md" || line == "@./AGENTS.md")
}

/// `CLAUDE.md` files anywhere and `AGENTS.md` files below the root, skipping hidden folders
/// and dependency or build output.
fn find(dir: &Path, depth: usize, found: &mut Vec<PathBuf>) {
    if depth > 6 || found.len() > 50 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if file_name.starts_with('.')
                || matches!(
                    file_name.as_ref(),
                    "node_modules" | "target" | "dist" | "build" | "vendor" | "__pycache__"
                )
            {
                continue;
            }
            find(&path, depth + 1, found);
        } else if file_name == "CLAUDE.md" || (file_name == "AGENTS.md" && depth > 0) {
            found.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_thread_gets_every_instruction_file_once_a_codex_worker_what_codex_misses() {
        let dir = std::env::temp_dir().join(format!("brigadier-instr-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("app")).unwrap();
        std::fs::write(dir.join("AGENTS.md"), "Use pnpm.\n").unwrap();
        // A copy of the root AGENTS.md, and a nested file.
        std::fs::write(dir.join("CLAUDE.md"), "Use pnpm.\n").unwrap();
        std::fs::write(
            dir.join("app").join("CLAUDE.md"),
            "Keep components small.\n",
        )
        .unwrap();
        let thread = for_thread(&dir).await;
        assert!(thread.contains("### AGENTS.md\nUse pnpm."), "{thread}");
        assert_eq!(thread.matches("Use pnpm.").count(), 1, "{thread}");
        assert!(thread.contains("### app/CLAUDE.md (applies to files under app/)"));
        assert!(thread.contains("The user's request wins over them"));
        let worker = for_worker(ProviderKind::Codex, &dir).await;
        assert!(
            !worker.contains("Use pnpm."),
            "Codex loads the root file itself: {worker}"
        );
        assert!(worker.contains("Keep components small."));
        assert!(worker.contains("The task spec wins over them"));
        assert!(for_worker(ProviderKind::Claude, &dir).await.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
