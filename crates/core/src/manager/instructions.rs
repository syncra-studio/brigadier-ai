//! B13: workers honor the repository's instruction files whatever their vendor.
//!
//! What each CLI loads by itself (verified with claude 2.1.281 and codex 0.156.1):
//!
//! - **Claude Code**, with the adapter's `instructionFiles = claude-md-and-agents-md`: the root
//!   `CLAUDE.md`, `.claude/CLAUDE.md`, `.claude/rules/*.md` and the root `AGENTS.md` at start,
//!   and a subfolder's `CLAUDE.md` / `AGENTS.md` when it reads a file there, but only below its
//!   working folder. A Claude worker starts in the session's worker folder, outside its
//!   worktree (THREAD-PLAN.md Q8 lever 2), so its prompt carries all of them, like the thread's.
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
//! The session's thread works from its own folder outside the repository, so it gets the
//! instruction files of its workspace in its prompt, the root `AGENTS.md` too, whatever its
//! vendor (THREAD-PLAN.md Q1).
//!
//! Only what applies is loaded (BUILD-PLAN D1.6): the root's files (and `.claude`'s) always
//! come whole; a subfolder's come whole to a worker whose spec names a path under it, and to
//! a thread with the first file it reads or changes there (its output hook,
//! [`SessionManager::hook_rules`]; a Codex thread's runs from a home of its own, #121) or the
//! first `run` call there (#460). The
//! rest are listed by path, so none is unknown. A thread without the hook (a Codex login kept
//! in the system's keyring) gets them all.

use std::path::{Path, PathBuf};

use brigadier_providers::ProviderKind;

use super::SessionManager;
use crate::model::ConversationId;
use crate::tools::Role;

/// Bytes of instructions carried at most.
const MAX_BYTES: usize = 32 * 1024;

/// The instruction files `provider` does not load itself, formatted for the worker prompt.
/// Rules of a subfolder come whole only when the task's spec names a path under it; the
/// others are listed, to read before changing files there (BUILD-PLAN D1.6).
pub(crate) async fn for_worker(provider: ProviderKind, worktree: &Path, spec: &str) -> String {
    let (worktree, named) = (worktree.to_owned(), named_paths(spec));
    tokio::task::spawn_blocking(move || {
        let rules = found_rules(&worktree, provider == ProviderKind::Claude);
        let (pinned, left_out): (Vec<Rule>, Vec<Rule>) = rules.into_iter().partition(|rule| {
            rule.folder
                .as_ref()
                .is_none_or(|folder| named.iter().any(|path| Path::new(path).starts_with(folder)))
        });
        format_rules(
            &pinned,
            &left_out,
            "The task spec wins over them",
            "Read one before you change files under its folder.",
        )
    })
    .await
    .unwrap_or_default()
}

/// The workspace's instruction files, formatted for the thread's prompt: its working
/// directory is its own folder outside the repository, so neither CLI finds them itself. A
/// thread with its output hook (`hooked`) gets the root's whole and a subfolder's listed: the
/// hook gives it a subfolder's rules with the first file it reads or changes there
/// ([`rules_for`], BUILD-PLAN D1.6). A thread without one gets them all.
pub(crate) async fn for_thread(workspace: &Path, hooked: bool) -> String {
    let workspace = workspace.to_owned();
    tokio::task::spawn_blocking(move || {
        let rules = found_rules(&workspace, true);
        let (pinned, left_out): (Vec<Rule>, Vec<Rule>) = if hooked {
            rules.into_iter().partition(|rule| rule.folder.is_none())
        } else {
            (rules, Vec::new())
        };
        format_rules(
            &pinned,
            &left_out,
            "The user's request wins over them",
            SUBFOLDER_RULES_COME,
        )
    })
    .await
    .unwrap_or_default()
}

/// How the thread gets a subfolder's rules it was only told of.
pub(crate) const SUBFOLDER_RULES_COME: &str = "Each comes to you with the first file you read or change under its folder; before you change files there with the shell, read it yourself.";

/// The rules of the subfolders that hold `touched` (a path in `workspace`, or through its
/// real path), outermost first: (their path in the workspace, their section).
pub(crate) fn rules_for(workspace: &Path, touched: &Path) -> Vec<(String, String)> {
    let real = brigadier_providers::policy::real_path(workspace);
    let Some(rel) = [Some(workspace), real.as_deref()]
        .into_iter()
        .flatten()
        .find_map(|root| touched.strip_prefix(root).ok())
    else {
        return Vec::new();
    };
    let mut rules: Vec<Rule> = found_rules(workspace, true)
        .into_iter()
        .filter(|rule| {
            rule.folder
                .as_ref()
                .is_some_and(|folder| rel.starts_with(folder))
        })
        .collect();
    rules.sort_by_key(|rule| rule.rel.components().count());
    rules
        .iter()
        .map(|rule| (rule.rel.display().to_string(), rule.section()))
        .collect()
}

impl SessionManager {
    /// What a thread's output hook asks after a file tool's call (`brigadierd hook
    /// post-tool-use`; a Codex thread's after a shell call or patch, for each file it names):
    /// the rules of the touched file's subfolders its session hasn't had
    /// yet, each once per CLI session (a new session's hook has a new grant). `None` from
    /// the grant: it isn't a live hook grant.
    pub async fn hook_rules(&self, grant: &str, touched: &str) -> Option<Option<String>> {
        let Some(Role::OutputHook { conversation_id }) = self.grants.resolve(grant) else {
            return None;
        };
        Some(
            self.rules_not_given(&conversation_id, grant, vec![PathBuf::from(touched)])
                .await,
        )
    }

    /// The rules of the subfolders that hold `paths` the thread of `id` hasn't had yet in its
    /// CLI session (whose hook has `grant`), now given: what its hook answers, and what its
    /// `run` and `run_unsandboxed` calls bring with their result (#460).
    pub(crate) async fn rules_not_given(
        &self,
        id: &ConversationId,
        grant: &str,
        paths: Vec<PathBuf>,
    ) -> Option<String> {
        let workspace = self.recorded_workspace(id)?.path;
        let found = tokio::task::spawn_blocking(move || {
            paths
                .iter()
                .flat_map(|path| rules_for(&workspace, path))
                .collect::<Vec<_>>()
        })
        .await
        .unwrap_or_default();
        let new: Vec<String> = {
            let mut given = self.rules_given.lock().unwrap();
            let (of, had) = given.entry(id.clone()).or_default();
            if of != grant {
                *of = grant.to_owned();
                had.clear();
            }
            found
                .into_iter()
                .filter(|(rel, _)| had.insert(rel.clone()))
                .map(|(_, section)| section)
                .collect()
        };
        if new.is_empty() {
            return None;
        }
        Some(format!(
            "[Brigadier] Repository rules for the folder you just worked in. Follow them for the files under it; the user's request wins over them, and a file in a subfolder wins over one higher up:{}",
            new.concat()
        ))
    }
}

/// Paths of one shell call asked about for their rules at most.
pub const COMMAND_PATHS_MAX: usize = 16;

/// The files and folders a shell command names that are there: `cwd`, the folders it `cd`s
/// to, and each of its words, as the shell reads them, from `cwd` or the folder a `cd` before
/// it went to. Options, URLs and
/// words the shell expands itself are left out.
pub fn command_paths(command: &str, cwd: &Path) -> Vec<PathBuf> {
    let mut dir = cwd.to_owned();
    // The folder it runs in, which a search there reads without naming it.
    let mut found: Vec<PathBuf> = vec![dir.clone()];
    let mut words = command_words(command).into_iter();
    while let Some(word) = words.next() {
        if matches!(word.as_str(), "cd" | "pushd") {
            // The folder it went to counts too: a search there names no path of its own.
            if let Some(to) = words.next().map(|to| lexical(&dir.join(to)))
                && to.is_dir()
            {
                if !found.contains(&to) {
                    found.push(to.clone());
                }
                dir = to;
            }
            continue;
        }
        if word.is_empty()
            || word.starts_with('-')
            || word.contains("://")
            || word.contains(['*', '?', '$', '`', '{', '~'])
        {
            continue;
        }
        let path = lexical(&dir.join(&word));
        if path.exists() && !found.contains(&path) {
            found.push(path);
        }
    }
    found
}

/// A command's words: quotes removed, a backslash keeping the next character, and the shell's
/// operators ending a word.
fn command_words(command: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut chars = command.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => word.extend(chars.by_ref().take_while(|&c| c != '\'')),
            '"' => {
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => word.extend(chars.next()),
                        c => word.push(c),
                    }
                }
            }
            '\\' => word.extend(chars.next()),
            c if c.is_whitespace() || ";|&<>()".contains(c) => {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            }
            c => word.push(c),
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

/// `path` without its `.` and `..` parts, as the daemon matches folders by name: a `..`
/// takes the part before it away, so `apps/../lib/x.rs` is `lib`'s.
pub fn lexical(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut clean = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                clean.pop();
            }
            other => clean.push(other),
        }
    }
    clean
}

/// One instruction file to carry.
struct Rule {
    /// Its path in the workspace.
    rel: PathBuf,
    /// The subfolder whose files it applies to; `None` for the root's (and `.claude`'s).
    folder: Option<PathBuf>,
    content: String,
}

impl Rule {
    fn section(&self) -> String {
        let scope = self
            .folder
            .as_ref()
            .map(|p| format!(" (applies to files under {}/)", p.display()))
            .unwrap_or_default();
        format!("\n\n### {}{scope}\n{}", self.rel.display(), self.content)
    }
}

/// The paths a task's spec names (words with a `/` in them, relative to the worktree).
fn named_paths(spec: &str) -> Vec<String> {
    spec.split(|c: char| {
        c.is_whitespace() || matches!(c, '`' | '"' | '\'' | '(' | ')' | ',' | '[' | ']')
    })
    .map(|word| {
        let word = word
            .trim_end_matches(['.', ':', ';'])
            .trim_start_matches("./");
        word.split(':').next().unwrap_or(word)
    })
    .filter(|word| word.contains('/') && !word.starts_with('/') && !word.contains(".."))
    .map(str::to_owned)
    .collect()
}

/// The instruction files in `worktree` (`root_agents`: its root `AGENTS.md` too), each once.
fn found_rules(worktree: &Path, root_agents: bool) -> Vec<Rule> {
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
    let mut rules = Vec::new();
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
        let rel = path.strip_prefix(worktree).unwrap_or(&path).to_owned();
        let folder = rel
            .parent()
            .filter(|p| !p.as_os_str().is_empty() && !p.starts_with(".claude"))
            .map(Path::to_owned);
        rules.push(Rule {
            rel,
            folder,
            content: content.to_owned(),
        });
    }
    rules
}

/// The prompt's section: the `pinned` rules whole, then the `left_out` ones named, with `how`
/// they are had; `precedence` says what wins over them.
fn format_rules(pinned: &[Rule], left_out: &[Rule], precedence: &str, how: &str) -> String {
    let mut text = String::new();
    for rule in pinned {
        let block = rule.section();
        if text.len() + block.len() > MAX_BYTES {
            text.push_str("\n\n[more instruction files were left out for length; read them in the repository]");
            break;
        }
        text.push_str(&block);
    }
    if !left_out.is_empty() {
        let names: Vec<String> = left_out
            .iter()
            .map(|rule| {
                let folder = rule.folder.as_deref().unwrap_or(Path::new("."));
                format!("{} (files under {}/)", rule.rel.display(), folder.display())
            })
            .collect();
        text.push_str(&format!(
            "\n\nRules for subfolders, not shown here: {}. {how}",
            names.join(", ")
        ));
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

    /// A folder in the temp directory, removed when dropped however the test ends.
    struct Temp(PathBuf);

    impl std::ops::Deref for Temp {
        type Target = Path;

        fn deref(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn the_thread_gets_every_instruction_file_once_a_codex_worker_what_codex_misses() {
        let dir =
            Temp(std::env::temp_dir().join(format!("brigadier-instr-{}", uuid::Uuid::new_v4())));
        std::fs::create_dir_all(dir.join("app")).unwrap();
        std::fs::write(dir.join("AGENTS.md"), "Use pnpm.\n").unwrap();
        // A copy of the root AGENTS.md, and a nested file.
        std::fs::write(dir.join("CLAUDE.md"), "Use pnpm.\n").unwrap();
        std::fs::write(
            dir.join("app").join("CLAUDE.md"),
            "Keep components small.\n",
        )
        .unwrap();
        // A thread without a hook to bring a subfolder's rules gets them all.
        let thread = for_thread(&dir, false).await;
        assert!(thread.contains("### AGENTS.md\nUse pnpm."), "{thread}");
        assert_eq!(thread.matches("Use pnpm.").count(), 1, "{thread}");
        assert!(thread.contains("### app/CLAUDE.md (applies to files under app/)"));
        assert!(thread.contains("The user's request wins over them"));
        // A worker whose spec names a path under app/ gets app's rules whole.
        let spec = "Fix app/button.tsx.";
        let worker = for_worker(ProviderKind::Codex, &dir, spec).await;
        assert!(
            !worker.contains("Use pnpm."),
            "Codex loads the root file itself: {worker}"
        );
        assert!(worker.contains("Keep components small."));
        assert!(worker.contains("The task spec wins over them"));
        // A Claude worker starts outside its worktree: it gets every file, the root's too.
        let claude = for_worker(ProviderKind::Claude, &dir, spec).await;
        assert_eq!(claude.matches("Use pnpm.").count(), 1, "{claude}");
        assert!(claude.contains("Keep components small."));
        assert!(claude.contains("The task spec wins over them"));
    }

    /// Done when (D1.6, in part): a Claude thread in a monorepo starts with the root's rules
    /// whole and a sub-package's only named, so its first call is smaller; the sub-package's
    /// rules come with the first file it touches there, outermost first. A worker on another
    /// package gets that package's rules and only names the rest.
    #[tokio::test]
    async fn only_the_rules_that_apply_are_loaded_and_the_rest_are_listed() {
        let dir =
            Temp(std::env::temp_dir().join(format!("brigadier-instr-{}", uuid::Uuid::new_v4())));
        let web = dir.join("apps").join("web");
        let api = dir.join("packages").join("api");
        std::fs::create_dir_all(web.join("src")).unwrap();
        std::fs::create_dir_all(&api).unwrap();
        std::fs::write(dir.join("AGENTS.md"), "Use pnpm.\n").unwrap();
        std::fs::write(
            dir.join("apps").join("AGENTS.md"),
            "Apps ship on Fridays.\n",
        )
        .unwrap();
        let web_rules = "Web components stay under 200 lines.\n".repeat(200);
        std::fs::write(web.join("CLAUDE.md"), &web_rules).unwrap();
        std::fs::write(api.join("CLAUDE.md"), "Every handler logs its route.\n").unwrap();
        let lean = for_thread(&dir, true).await;
        let whole = for_thread(&dir, false).await;
        assert!(lean.contains("### AGENTS.md\nUse pnpm."), "{lean}");
        assert!(!lean.contains("Web components"), "{lean}");
        assert!(!lean.contains("Every handler"), "{lean}");
        for listed in [
            "apps/AGENTS.md (files under apps/)",
            "apps/web/CLAUDE.md (files under apps/web/)",
            "packages/api/CLAUDE.md (files under packages/api/)",
        ] {
            assert!(lean.contains(listed), "{listed}: {lean}");
        }
        assert!(lean.contains(SUBFOLDER_RULES_COME));
        assert!(
            lean.len() * 4 < whole.len(),
            "smaller first call: {} bytes instead of {}",
            lean.len(),
            whole.len()
        );
        // Touching a file under apps/web brings apps' rules, then apps/web's.
        let touched = rules_for(&dir, &web.join("src").join("page.tsx"));
        let names: Vec<&str> = touched.iter().map(|(rel, _)| rel.as_str()).collect();
        assert_eq!(names, ["apps/AGENTS.md", "apps/web/CLAUDE.md"]);
        assert!(
            touched[1]
                .1
                .contains("Web components stay under 200 lines.")
        );
        assert!(rules_for(&dir, &dir.join("README.md")).is_empty());
        assert!(rules_for(Path::new("/elsewhere"), &web.join("x")).is_empty());
        // A worker on packages/api gets its rules whole and the web's only named.
        let worker = for_worker(
            ProviderKind::Claude,
            &dir,
            "Add a health route in `packages/api/routes.ts`.",
        )
        .await;
        assert!(worker.contains("Every handler logs its route."), "{worker}");
        assert!(!worker.contains("Web components"), "{worker}");
        assert!(worker.contains("apps/web/CLAUDE.md (files under apps/web/)"));
        assert!(worker.contains("Read one before you change files under its folder."));
    }
}
