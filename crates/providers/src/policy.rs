//! Brigadier's approval policy for CLI sessions.
//!
//! What a session may do is set by its permission level and enforced by its CLI (PLAN.md §5):
//! Full access runs without a sandbox and never asks; Approve for me runs inside the CLI's OS
//! sandbox and lets the CLI's own reviewer settle what leaves it; Ask for approval asks the
//! user each time a command needs more than the sandbox allows. Requests that still reach
//! Brigadier are routed by [`route`]; the user's "for this session" answer covers similar later
//! requests for the rest of the conversation ([`Similar`]).

use crate::model::{Access, ApprovalKind, ApprovalRequest};

/// What Brigadier does with an approval request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// Answer yes on the user's behalf.
    Allow,
    /// Answer no on the user's behalf.
    Deny,
    /// Only the user can answer.
    AskUser,
}

/// How approvals are answered for a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalMode {
    /// Approve for me: allow what stays inside the session's access, ask the user otherwise.
    Delegated,
    /// Decline everything (a read-only session such as the orchestrator).
    DeclineAll,
}

/// Decides who answers `request`.
pub fn route(request: &ApprovalRequest, access: &Access, mode: ApprovalMode) -> Route {
    match mode {
        ApprovalMode::DeclineAll => Route::Deny,
        ApprovalMode::Delegated => route_delegated(request, access),
    }
}

/// [`route`] under Approve for me.
fn route_delegated(request: &ApprovalRequest, access: &Access) -> Route {
    if request.escalation || request.kind == ApprovalKind::Permissions {
        return Route::AskUser;
    }
    // Reaching a host from a sandbox without network (Ask for approval) is the user's call.
    if request.tool == NETWORK_TOOL {
        return match access {
            Access::Scoped { network: true, .. } | Access::Workspace { .. } | Access::Full => {
                Route::Allow
            }
            Access::Scoped { network: false, .. } | Access::ReadOnly => Route::AskUser,
        };
    }
    match access {
        Access::ReadOnly => Route::AskUser,
        Access::Workspace { .. } | Access::Full => Route::Allow,
        // The OS sandbox confines commands; file tools outside it may only write where the
        // sandbox would let a command write.
        Access::Scoped {
            write_cwd,
            writable_roots,
            ..
        } => {
            if request.kind != ApprovalKind::FileChange {
                return Route::Allow;
            }
            // Compared as the file system resolves them: `/tmp/x` and `/private/tmp/x` are one
            // folder on macOS.
            let cwd = request
                .cwd
                .as_deref()
                .and_then(|cwd| real_path(std::path::Path::new(cwd)));
            let roots: Vec<std::path::PathBuf> = writable_roots
                .iter()
                .filter_map(|root| real_path(root))
                .collect();
            let writable = |path: &std::path::Path| {
                real_path(path).is_some_and(|path| {
                    roots.iter().any(|root| path.starts_with(root))
                        || (*write_cwd && cwd.as_ref().is_some_and(|cwd| path.starts_with(cwd)))
                })
            };
            if !request.paths.is_empty()
                && request
                    .paths
                    .iter()
                    .all(|path| writable(std::path::Path::new(path)))
            {
                Route::Allow
            } else {
                Route::AskUser
            }
        }
    }
}

/// `path` as the file system resolves it, symlinks included (`/tmp` → `/private/tmp` on
/// macOS), also for a file not created yet: its deepest existing folder is resolved and the
/// rest appended. `None` for a relative path or one with `..` in it, which could lead anywhere.
pub fn real_path(path: &std::path::Path) -> Option<std::path::PathBuf> {
    use std::path::Component;
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return None;
    }
    let mut rest = Vec::new();
    let mut existing = path;
    loop {
        if let Ok(real) = existing.canonicalize() {
            return Some(rest.iter().rev().fold(real, |path, part| path.join(part)));
        }
        rest.push(existing.file_name()?);
        existing = existing.parent()?;
    }
}

/// The script of a `sh -c '…'` wrapper (`/bin/zsh -lc 'npm test'` → `npm test`), else the
/// command itself.
pub fn unwrapped_command(command: &str) -> String {
    let commands = simple_commands(command);
    if let [inner @ .., outer] = commands.as_slice()
        && let Some(script) = shell_script(outer)
        && simple_commands(&script).len() == inner.len()
    {
        return script.trim().to_owned();
    }
    command.trim().to_owned()
}

/// Drops environment assignments and transparent wrappers (`env`, `sudo`, `command`, …).
fn strip_prefixes(words: &[String]) -> &[String] {
    const WRAPPERS: &[&str] = &["env", "sudo", "command", "exec", "nohup", "time", "nice"];
    let mut index = 0;
    while let Some(word) = words.get(index) {
        let assignment = word
            .split_once('=')
            .is_some_and(|(name, _)| !name.is_empty() && !name.starts_with('-'));
        let wrapper = WRAPPERS.contains(&word.as_str());
        let wrapper_option = index > 0 && word.starts_with('-');
        if assignment || wrapper || wrapper_option {
            index += 1;
        } else {
            break;
        }
    }
    &words[index..]
}

/// Splits a command line into simple commands (word lists), descending into `sh -c` scripts,
/// `$(…)`, backticks and parentheses.
fn simple_commands(line: &str) -> Vec<Vec<String>> {
    let mut commands = Vec::new();
    collect_commands(line, &mut commands, 0);
    commands
}

fn collect_commands(line: &str, commands: &mut Vec<Vec<String>>, depth: usize) {
    if depth > 4 {
        return;
    }
    // Simple commands at this level; nested ones go straight to `commands`.
    let mut local: Vec<Vec<String>> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut plain_word = true;
    let mut redirection_target = false;
    let mut chars = line.chars().peekable();

    let finish_word = |word: &mut String,
                       in_word: &mut bool,
                       plain_word: &mut bool,
                       redirection_target: &mut bool,
                       current: &mut Vec<String>| {
        if *in_word {
            let word = std::mem::take(word);
            if !*redirection_target {
                current.push(word);
            }
            *in_word = false;
            *redirection_target = false;
            *plain_word = true;
        }
    };

    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                plain_word = false;
                for c in chars.by_ref() {
                    if c == '\'' {
                        break;
                    }
                    word.push(c);
                }
            }
            '"' => {
                in_word = true;
                plain_word = false;
                let mut inner = String::new();
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => {
                            if let Some(next) = chars.next() {
                                inner.push(next);
                            }
                        }
                        _ => inner.push(c),
                    }
                }
                if inner.contains("$(") || inner.contains('`') {
                    collect_commands(&inner, commands, depth + 1);
                }
                word.push_str(&inner);
            }
            '\\' => {
                in_word = true;
                plain_word = false;
                if let Some(next) = chars.next() {
                    word.push(next);
                }
            }
            '$' if chars.peek() == Some(&'(') => {
                chars.next();
                let inner = take_balanced(&mut chars);
                collect_commands(&inner, commands, depth + 1);
                in_word = true;
            }
            '`' => {
                let inner: String = chars.by_ref().take_while(|c| *c != '`').collect();
                collect_commands(&inner, commands, depth + 1);
                in_word = true;
            }
            '(' | '{' if !in_word => {
                let inner = if c == '(' {
                    take_balanced(&mut chars)
                } else {
                    chars.by_ref().take_while(|c| *c != '}').collect()
                };
                collect_commands(&inner, commands, depth + 1);
            }
            '<' | '>' => {
                // Only unquoted shell syntax redirects. A quoted "> text" or escaped
                // \> is an ordinary argument, and must survive in an approval pass.
                if plain_word && in_word && word.chars().all(|c| c.is_ascii_digit()) {
                    word.clear();
                    in_word = false;
                }
                finish_word(
                    &mut word,
                    &mut in_word,
                    &mut plain_word,
                    &mut redirection_target,
                    &mut current,
                );
                while chars
                    .peek()
                    .is_some_and(|c| matches!(c, '<' | '>' | '&' | '|'))
                {
                    chars.next();
                }
                redirection_target = true;
            }
            ';' | '&' | '|' | '\n' => {
                finish_word(
                    &mut word,
                    &mut in_word,
                    &mut plain_word,
                    &mut redirection_target,
                    &mut current,
                );
                if !current.is_empty() {
                    local.push(std::mem::take(&mut current));
                }
            }
            c if c.is_whitespace() => finish_word(
                &mut word,
                &mut in_word,
                &mut plain_word,
                &mut redirection_target,
                &mut current,
            ),
            c => {
                in_word = true;
                word.push(c);
            }
        }
    }
    finish_word(
        &mut word,
        &mut in_word,
        &mut plain_word,
        &mut redirection_target,
        &mut current,
    );
    if !current.is_empty() {
        local.push(current);
    }

    // `sh -c 'script'` and friends: the script is where the commands are.
    for words in &local {
        if let Some(script) = shell_script(words) {
            collect_commands(&script, commands, depth + 1);
        }
    }
    commands.extend(local);
}

fn take_balanced(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut depth = 1;
    let mut inner = String::new();
    for c in chars.by_ref() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
        inner.push(c);
    }
    inner
}

/// The script of `bash -c '…'`, `zsh -lc '…'` and similar.
fn shell_script(words: &[String]) -> Option<String> {
    let words = strip_prefixes(words);
    let (program, rest) = words.split_first()?;
    let name = program.rsplit('/').next().unwrap_or(program);
    if !matches!(name, "sh" | "bash" | "zsh" | "dash" | "ksh" | "fish") {
        return None;
    }
    let flag = rest
        .iter()
        .position(|word| word.starts_with('-') && !word.starts_with("--") && word.contains('c'))?;
    rest.get(flag + 1).cloned()
}

/// The tool name Claude asks under for network access from inside its sandbox (its input
/// names the `host`).
pub const NETWORK_TOOL: &str = "SandboxNetworkAccess";

/// Programs that only move between folders or filter what another command prints: a command
/// line's `cd repo && …` or `… | head -5` doesn't count when matching it against "Allow
/// similar commands".
const NEUTRAL: &[&str] = &[
    "cd", "pushd", "popd", "head", "tail", "grep", "sort", "uniq", "wc", "cut", "tr", "jq", "true",
    "echo",
];

/// What "Allow … for this session" covers for `command`: its first program and, when the next
/// word is a plain subcommand, that word too (`git push origin main` → `git push`, `curl -sI
/// https://…` → `curl`, `cargo test -p core` → `cargo test`). `None` for a line with no
/// program in it.
pub fn command_prefix(command: &str) -> Option<String> {
    let words = simple_commands(&unwrapped_command(command))
        .into_iter()
        .map(|words| strip_prefixes(&words).to_vec())
        .find(|words| !words.is_empty() && !NEUTRAL.contains(&program_name(&words[0])))?;
    Some(prefix_words(&words).join(" "))
}

/// The words of `command`'s prefix: the program's name, then a plain subcommand if there is one.
fn prefix_words(words: &[String]) -> Vec<String> {
    let mut prefix = vec![program_name(&words[0]).to_owned()];
    if let Some(next) = words.get(1)
        && next
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_lowercase())
        && next
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    {
        prefix.push(next.clone());
    }
    prefix
}

/// A program's name without its folder (`/usr/bin/git` → `git`).
fn program_name(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

/// What the user allowed "for this session" in one conversation: commands that start with the
/// same words, network access to the same hosts, and file changes inside a session's own
/// workspace (its checkout or worktree). Never persisted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Similar {
    prefixes: Vec<Vec<String>>,
    hosts: Vec<String>,
    folders: Vec<std::path::PathBuf>,
}

impl Similar {
    /// Allows what is similar to `request` from now on. Does nothing for a request that has
    /// nothing to be similar to (a permissions request, a file change without a
    /// [`workspace_grant`]).
    pub fn allow(&mut self, request: &ApprovalRequest) {
        if let Some(host) = network_host(request) {
            if !self.hosts.contains(&host) {
                self.hosts.push(host);
            }
        } else if request.kind == ApprovalKind::FileChange {
            let Some(folder) = request.grant.as_deref() else {
                return;
            };
            let Some(folder) = resolved_path(std::path::Path::new(folder)) else {
                return;
            };
            if !self.folders.contains(&folder) {
                self.folders.push(folder);
            }
        } else if let Some(prefix) = request.command.as_deref().and_then(command_prefix) {
            let words: Vec<String> = prefix.split(' ').map(str::to_owned).collect();
            if !self.prefixes.contains(&words) {
                self.prefixes.push(words);
            }
        }
    }

    /// Adds everything `other` allows.
    pub fn merge(&mut self, other: Similar) {
        for prefix in other.prefixes {
            if !self.prefixes.contains(&prefix) {
                self.prefixes.push(prefix);
            }
        }
        for host in other.hosts {
            if !self.hosts.contains(&host) {
                self.hosts.push(host);
            }
        }
        for folder in other.folders {
            if !self.folders.contains(&folder) {
                self.folders.push(folder);
            }
        }
    }

    /// Whether `request` is similar to one the user allowed: network access to an allowed
    /// host, a file change inside an allowed workspace, or a command line whose every command
    /// starts with allowed words.
    pub fn covers(&self, request: &ApprovalRequest) -> bool {
        if let Some(host) = network_host(request) {
            return self.hosts.contains(&host);
        }
        if request.kind == ApprovalKind::FileChange {
            return self
                .folders
                .iter()
                .any(|folder| workspace_grant(request, folder).is_some());
        }
        if request.kind != ApprovalKind::Command || self.prefixes.is_empty() {
            return false;
        }
        let Some(command) = &request.command else {
            return false;
        };
        // A grant is for running a program, not for writing files with the shell's help.
        if redirects_to_file(command) {
            return false;
        }
        let commands: Vec<Vec<String>> = simple_commands(command)
            .into_iter()
            .map(|words| strip_prefixes(&words).to_vec())
            .filter(|words| !words.is_empty())
            .collect();
        let mut judged = 0;
        for words in &commands {
            if neutral(words) {
                continue;
            }
            // A shell wrapper is judged by the commands of its script.
            if shell_script(words).is_some() {
                continue;
            }
            judged += 1;
            let covered = self.prefixes.iter().any(|prefix| {
                program_name(&words[0]) == prefix[0]
                    && prefix[1..]
                        .iter()
                        .zip(&words[1..])
                        .all(|(wanted, word)| wanted == word)
                    && words.len() >= prefix.len()
            });
            if !covered {
                return false;
            }
        }
        judged > 0
    }
}

/// Whether a simple command only moves between folders or filters what it is given ([`NEUTRAL`]),
/// writing no file of its own: `sort -o out` and `uniq in out` write one.
fn neutral(words: &[String]) -> bool {
    let program = program_name(&words[0]);
    if !NEUTRAL.contains(&program) {
        return false;
    }
    let args = &words[1..];
    match program {
        "sort" => !args.iter().any(|arg| {
            arg.starts_with("--output")
                || (arg.starts_with('-') && !arg.starts_with("--") && arg.contains('o'))
        }),
        "uniq" => args.iter().filter(|arg| !arg.starts_with('-')).count() < 2,
        _ => true,
    }
}

/// Whether `command` sends output into a file with the shell's `>` or `>>` (outside quotes),
/// other than `/dev/null` and the like or another file descriptor (`2>&1`).
fn redirects_to_file(command: &str) -> bool {
    let mut chars = command.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                chars.next();
            }
            '\'' => {
                for c in chars.by_ref() {
                    if c == '\'' {
                        break;
                    }
                }
            }
            '"' => {
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => {
                            chars.next();
                        }
                        _ => {}
                    }
                }
            }
            '>' => {
                while chars.peek().is_some_and(|c| matches!(c, '>' | '|')) {
                    chars.next();
                }
                if chars.peek() == Some(&'&') {
                    // `>&2` duplicates a descriptor; `>&file` is zsh's and bash's `>file 2>&1`.
                    chars.next();
                    let target: String = chars
                        .by_ref()
                        .take_while(|c| !c.is_whitespace() && !matches!(c, ';' | '|' | '&'))
                        .collect();
                    if !target.chars().all(|c| c.is_ascii_digit() || c == '-') {
                        return true;
                    }
                    continue;
                }
                while chars.peek().is_some_and(|c| c.is_whitespace()) {
                    chars.next();
                }
                let target: String = chars
                    .by_ref()
                    .take_while(|c| !c.is_whitespace() && !matches!(c, ';' | '|' | '&' | ')'))
                    .collect();
                let target = target.trim_matches(|c| c == '"' || c == '\'');
                if !matches!(target, "/dev/null" | "/dev/stdout" | "/dev/stderr") {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

/// What "for this session" would allow for a file change asked for in `workspace`: the
/// workspace itself, when every path the request names lies inside it as the file system
/// resolves them (a symlink leading out is outside). `None` for anything else, and for a path
/// in a `.git` folder, whose hooks and config run code: those ask every time.
pub fn workspace_grant(request: &ApprovalRequest, workspace: &std::path::Path) -> Option<String> {
    use std::path::Component;
    if request.kind != ApprovalKind::FileChange || request.paths.is_empty() {
        return None;
    }
    let root = resolved_path(workspace)?;
    let git_dirs = git_metadata_dirs(&root)?;
    let in_git = |path: &std::path::Path| {
        path.components()
            .any(|part| matches!(part, Component::Normal(name) if name == ".git"))
    };
    let inside = |path: &str| {
        let path = std::path::Path::new(path);
        !in_git(path)
            && resolved_path(path).is_some_and(|real| {
                !git_dirs.iter().any(|git| real.starts_with(git))
                    && real.strip_prefix(&root).is_ok_and(|rest| !in_git(rest))
            })
    };
    request
        .paths
        .iter()
        .all(|path| inside(path))
        .then(|| workspace.display().to_string())
}

/// Git can keep its administrative folders under names other than `.git`. Follow the
/// checkout's gitfile and optional `commondir`; unreadable metadata grants nothing.
fn git_metadata_dirs(workspace: &std::path::Path) -> Option<Vec<std::path::PathBuf>> {
    let mut dirs = Vec::new();
    for ancestor in workspace.ancestors() {
        let dotgit = ancestor.join(".git");
        match dotgit.symlink_metadata() {
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return None,
            Ok(_) => {}
        }
        let git = if dotgit.is_dir() {
            dotgit.canonicalize().ok()?
        } else {
            let text = std::fs::read_to_string(&dotgit).ok()?;
            ancestor
                .join(text.trim().strip_prefix("gitdir: ")?)
                .canonicalize()
                .ok()?
        };
        match std::fs::read_to_string(git.join("commondir")) {
            Ok(common) => dirs.push(git.join(common.trim()).canonicalize().ok()?),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return None,
        }
        dirs.push(git);
        break;
    }
    Some(dirs)
}

/// [`real_path`] for a grant: only what is missing may stay unresolved. A symlink that leads
/// nowhere (a write through it lands wherever it points) or a part that can't be read gives
/// `None`.
fn resolved_path(path: &std::path::Path) -> Option<std::path::PathBuf> {
    use std::path::Component;
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return None;
    }
    let mut rest = Vec::new();
    let mut existing = path;
    loop {
        match existing.symlink_metadata() {
            Ok(_) => {
                let real = existing.canonicalize().ok()?;
                return Some(rest.iter().rev().fold(real, |path, part| path.join(part)));
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                rest.push(existing.file_name()?);
                existing = existing.parent()?;
            }
            Err(_) => return None,
        }
    }
}

/// The host of a request for network access from inside the sandbox.
fn network_host(request: &ApprovalRequest) -> Option<String> {
    if request.tool != NETWORK_TOOL {
        return None;
    }
    request.grant.clone()
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;

    /// A folder `real/scratch/outputs` and a symlink `link` → `real`, as `/tmp` → `/private/tmp`.
    struct Linked {
        base: PathBuf,
    }

    impl Linked {
        fn new(name: &str) -> Self {
            let base = std::env::temp_dir()
                .join(format!("brigadier-policy-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&base);
            std::fs::create_dir_all(base.join("real/scratch/outputs")).unwrap();
            #[cfg(unix)]
            std::os::unix::fs::symlink(base.join("real"), base.join("link")).unwrap();
            #[cfg(windows)]
            std::os::windows::fs::symlink_dir(base.join("real"), base.join("link")).unwrap();
            Self { base }
        }

        fn real(&self) -> PathBuf {
            self.base.join("real").canonicalize().unwrap()
        }

        fn link(&self) -> PathBuf {
            self.base.join("link")
        }
    }

    impl Drop for Linked {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.base);
        }
    }

    fn write(path: &Path) -> ApprovalRequest {
        ApprovalRequest {
            id: "1".into(),
            kind: ApprovalKind::FileChange,
            tool: "Write".into(),
            command: None,
            cwd: None,
            paths: vec![path.display().to_string()],
            reason: None,
            escalation: false,
            input: None,
            grant: None,
        }
    }

    fn scoped(root: PathBuf) -> Access {
        Access::Scoped {
            write_cwd: false,
            writable_roots: vec![root],
            network: false,
            deny_read: Vec::new(),
            unix_sockets: Vec::new(),
        }
    }

    fn routed(path: &Path, root: PathBuf) -> Route {
        route(&write(path), &scoped(root), ApprovalMode::Delegated)
    }

    #[test]
    fn a_new_file_resolves_through_its_existing_folders() {
        let dirs = Linked::new("new");
        assert_eq!(
            real_path(&dirs.link().join("scratch/outputs/a/b.md")),
            Some(dirs.real().join("scratch/outputs/a/b.md"))
        );
    }

    #[test]
    fn either_spelling_of_a_linked_root_is_inside_it() {
        let dirs = Linked::new("spell");
        let file = "scratch/outputs/navigation.md";
        // The root as configured (through the link), the file as the CLI resolved it; and the
        // other way round.
        assert_eq!(
            routed(&dirs.real().join(file), dirs.link().join("scratch")),
            Route::Allow
        );
        assert_eq!(
            routed(&dirs.link().join(file), dirs.real().join("scratch")),
            Route::Allow
        );
        assert_eq!(
            routed(&dirs.link().join(file), dirs.link().join("scratch")),
            Route::Allow
        );
    }

    #[test]
    fn a_path_outside_or_climbing_out_is_not() {
        let dirs = Linked::new("out");
        let root = dirs.link().join("scratch");
        assert_eq!(
            routed(&dirs.real().join("elsewhere.md"), root.clone()),
            Route::AskUser
        );
        assert_eq!(
            routed(
                &dirs.link().join("scratch/outputs/../../x.md"),
                root.clone()
            ),
            Route::AskUser
        );
        assert_eq!(routed(Path::new("scratch/x.md"), root), Route::AskUser);
    }

    fn command(line: &str, cwd: Option<&Path>, escalation: bool) -> ApprovalRequest {
        ApprovalRequest {
            kind: ApprovalKind::Command,
            tool: "Bash".into(),
            command: Some(line.into()),
            cwd: cwd.map(|cwd| cwd.display().to_string()),
            paths: Vec::new(),
            escalation,
            ..write(Path::new("/"))
        }
    }

    #[test]
    fn allow_similar_covers_commands_with_the_same_first_words() {
        assert_eq!(
            command_prefix("curl -sI https://example.com").as_deref(),
            Some("curl")
        );
        assert_eq!(
            command_prefix("git push origin main").as_deref(),
            Some("git push")
        );
        assert_eq!(
            command_prefix("cd repo && FOO=1 cargo test -p core").as_deref(),
            Some("cargo test")
        );
        assert_eq!(
            command_prefix("/bin/zsh -lc 'npm install left-pad'").as_deref(),
            Some("npm install")
        );
        assert_eq!(
            command_prefix("python3 script.py").as_deref(),
            Some("python3")
        );

        let mut similar = Similar::default();
        similar.allow(&command("curl -sI https://example.com", None, true));
        for line in [
            "curl -sI https://example.com",
            "curl https://example.org/x | head -1",
            "cd /tmp && curl -L https://example.net",
            "/bin/zsh -lc 'curl -s https://example.com'",
        ] {
            assert!(similar.covers(&command(line, None, true)), "{line}");
        }
        for line in [
            "wget https://example.com",
            "curl x && rm -rf y",
            "cd /tmp",
            "",
        ] {
            assert!(!similar.covers(&command(line, None, true)), "{line}");
        }

        similar.allow(&command("git push origin main", None, true));
        assert!(similar.covers(&command("git push -u origin topic", None, true)));
        assert!(!similar.covers(&command("git reset --hard", None, true)));
    }

    #[test]
    fn network_access_asks_only_from_a_sandbox_without_network() {
        let request = ApprovalRequest {
            kind: ApprovalKind::Tool,
            tool: NETWORK_TOOL.into(),
            paths: Vec::new(),
            ..write(Path::new("/"))
        };
        let offline = scoped(PathBuf::from("/"));
        assert_eq!(
            route(&request, &offline, ApprovalMode::Delegated),
            Route::AskUser
        );
        let online = Access::Scoped {
            write_cwd: false,
            writable_roots: Vec::new(),
            network: true,
            deny_read: Vec::new(),
            unix_sockets: Vec::new(),
        };
        assert_eq!(
            route(&request, &online, ApprovalMode::Delegated),
            Route::Allow
        );
    }

    #[test]
    fn allow_similar_never_covers_writing_files_with_the_shell() {
        let mut similar = Similar::default();
        similar.allow(&command("curl https://example.com", None, true));
        for line in [
            "curl https://example.com 2>&1 | head -5",
            "curl -s https://example.com >/dev/null 2>&1",
            "curl https://example.com | sort | uniq -c",
            "echo '>' && curl https://example.com",
        ] {
            assert!(similar.covers(&command(line, None, true)), "{line}");
        }
        for line in [
            "curl https://example.com && sort -o /outside/file /tmp/input",
            "curl https://example.com && sort -uo /outside/file /tmp/input",
            "curl https://example.com && uniq /tmp/input /outside/file",
            "curl https://example.com && echo hi > ~/.zshrc",
            "curl https://example.com >> /outside/log",
            "curl https://example.com &> /outside/log",
            "curl https://example.com >&/outside/log",
        ] {
            assert!(!similar.covers(&command(line, None, true)), "{line}");
        }
    }

    #[test]
    fn allow_similar_covers_network_access_to_the_same_host() {
        let network = |host: &str| ApprovalRequest {
            kind: ApprovalKind::Tool,
            tool: NETWORK_TOOL.into(),
            grant: Some(host.into()),
            ..write(Path::new("/"))
        };
        let mut similar = Similar::default();
        assert!(!similar.covers(&network("example.com")));
        similar.allow(&network("example.com"));
        assert!(similar.covers(&network("example.com")));
        assert!(!similar.covers(&network("example.org")));
        // A file change has nothing to be similar to.
        similar.allow(&write(Path::new("/x")));
        assert!(!similar.covers(&write(Path::new("/x"))));
    }

    #[test]
    fn session_file_grants_exclude_git_common_dirs_with_custom_names() {
        let dirs = Linked::new("git-common");
        let workspace = dirs.real();
        let admin = workspace.join("admin");
        let common = workspace.join("metadata");
        std::fs::create_dir_all(&admin).unwrap();
        std::fs::create_dir_all(&common).unwrap();
        std::fs::write(workspace.join(".git"), "gitdir: admin\n").unwrap();
        std::fs::write(admin.join("commondir"), "../metadata\n").unwrap();
        assert!(workspace_grant(&write(&workspace.join("new.txt")), &workspace).is_some());
        for path in [admin.join("config"), common.join("hooks/pre-commit")] {
            assert_eq!(workspace_grant(&write(&path), &workspace), None);
        }
    }

    #[test]
    fn a_session_file_grant_covers_edits_inside_its_workspace_only() {
        let dirs = Linked::new("files");
        let workspace = dirs.link().join("scratch");
        let other = dirs.real().join("other");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::create_dir_all(workspace.join(".git/hooks")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&other, workspace.join("out")).unwrap();

        let inside = workspace.join("outputs/a.md");
        let mut asked = write(&inside);
        asked.grant = workspace_grant(&asked, &workspace);
        assert_eq!(asked.grant, Some(workspace.display().to_string()));
        let mut similar = Similar::default();
        similar.allow(&asked);

        // In the workspace, also a new file in new folders, and through either spelling.
        assert!(similar.covers(&write(&inside)));
        assert!(similar.covers(&write(&workspace.join("new/dir/b.md"))));
        assert!(similar.covers(&write(&dirs.real().join("scratch/c.md"))));
        // Outside it, climbing out, relative, through a symlink leading out, or in `.git`.
        for path in [
            other.join("a.md"),
            dirs.real().join("x.md"),
            workspace.join("outputs/../../x.md"),
            PathBuf::from("scratch/x.md"),
            workspace.join(".git/hooks/pre-commit"),
            workspace.join("sub/.git/config"),
        ] {
            assert!(!similar.covers(&write(&path)), "{}", path.display());
            assert_eq!(workspace_grant(&write(&path), &workspace), None);
        }
        #[cfg(unix)]
        {
            assert!(!similar.covers(&write(&workspace.join("out/a.md"))));
            // A symlink leading to a file outside that doesn't exist yet.
            std::os::unix::fs::symlink(other.join("missing.md"), workspace.join("dangling.md"))
                .unwrap();
            assert!(!similar.covers(&write(&workspace.join("dangling.md"))));
            std::os::unix::fs::symlink(other.join("gone"), workspace.join("gone")).unwrap();
            assert!(!similar.covers(&write(&workspace.join("gone/x.md"))));
        }
        // One path outside makes the whole request ask.
        let mut both = write(&inside);
        both.paths.push(other.join("a.md").display().to_string());
        assert!(!similar.covers(&both));
        // Another session's workspace is not this one's.
        let mut elsewhere = Similar::default();
        let mut asked = write(&other.join("a.md"));
        asked.grant = workspace_grant(&asked, &other);
        elsewhere.allow(&asked);
        assert!(!elsewhere.covers(&write(&inside)));
        // Handed over with the other grants.
        let mut merged = Similar::default();
        merged.merge(similar);
        assert!(merged.covers(&write(&inside)));
        // Commands and permissions requests are no file grant.
        assert_eq!(
            workspace_grant(&command("ls", None, false), &workspace),
            None
        );
    }
}
