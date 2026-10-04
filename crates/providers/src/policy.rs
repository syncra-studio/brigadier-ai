//! Brigadier's approval policy for CLI sessions.
//!
//! Workers run full-auto inside their CLI's OS sandbox, but "inside the sandbox" is not the same
//! as "authorized". Two kinds of request always reach a person:
//!
//! - actions that affect the outside world ([`ALWAYS_ASK`]: push, publish, deploy, cloud), at
//!   every permission level (PLAN.md §5);
//! - requests to leave or widen the sandbox.
//!
//! Each adapter makes its CLI ask Brigadier for these (Claude through ask rules and its
//! permission-prompt tool, Codex through its approval policy), and [`route`] decides what
//! Brigadier answers on the user's behalf.

use crate::model::{Access, ApprovalKind, ApprovalRequest};

/// Commands with effects outside the machine, as program + subcommand words. A command matches
/// when its program is the first word and the other words follow in order, with anything in
/// between (`git -C repo push` matches `git push`).
pub const ALWAYS_ASK: &[&[&str]] = &[
    &["git", "push"],
    &["git", "send-pack"],
    &["git", "send-email"],
    &["git", "lfs", "push"],
    &["gh", "gist", "create"],
    &["gh", "pr", "create"],
    &["gh", "pr", "merge"],
    &["gh", "pr", "close"],
    &["gh", "pr", "reopen"],
    &["gh", "pr", "edit"],
    &["gh", "pr", "comment"],
    &["gh", "pr", "review"],
    &["gh", "pr", "ready"],
    &["gh", "issue", "create"],
    &["gh", "issue", "close"],
    &["gh", "issue", "comment"],
    &["gh", "issue", "edit"],
    &["gh", "release"],
    &["gh", "repo", "create"],
    &["gh", "repo", "delete"],
    &["gh", "repo", "edit"],
    &["gh", "repo", "rename"],
    &["gh", "workflow", "run"],
    &["gh", "secret"],
    &["gh", "api"],
    &["npm", "publish"],
    &["npm", "unpublish"],
    &["pnpm", "publish"],
    &["yarn", "publish"],
    &["yarn", "npm", "publish"],
    &["bun", "publish"],
    &["cargo", "publish"],
    &["cargo", "yank"],
    &["twine", "upload"],
    &["poetry", "publish"],
    &["uv", "publish"],
    &["gem", "push"],
    &["pod", "trunk", "push"],
    &["docker", "push"],
    &["vercel"],
    &["netlify", "deploy"],
    &["fly", "deploy"],
    &["flyctl", "deploy"],
    &["wrangler", "deploy"],
    &["wrangler", "publish"],
    &["firebase", "deploy"],
    &["heroku"],
    &["railway", "up"],
    &["terraform", "apply"],
    &["terraform", "destroy"],
    &["pulumi", "up"],
    &["pulumi", "destroy"],
    &["kubectl", "apply"],
    &["kubectl", "delete"],
    &["helm", "install"],
    &["helm", "upgrade"],
    &["helm", "uninstall"],
    &["aws"],
    &["gcloud"],
    &["az"],
];

/// Claude Code permission rules that make every [`ALWAYS_ASK`] command prompt, even when an
/// allow rule or the sandbox's auto-allow would run it.
pub fn claude_ask_rules() -> Vec<String> {
    let mut rules = Vec::with_capacity(ALWAYS_ASK.len() * 2);
    for words in ALWAYS_ASK {
        rules.push(format!("Bash({} *)", words.join(" ")));
        if let [program, rest @ ..] = words
            && !rest.is_empty()
        {
            // Options between the program and its subcommand (`git -C repo push`).
            rules.push(format!("Bash({program} * {} *)", rest.join(" ")));
        }
    }
    rules
}

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
    /// Nobody is there to ask (an overnight run, PLAN.md §10.8): approve for the user
    /// whatever the session's access would let them approve, leaving the sandbox included;
    /// decline what only the user may do (outward actions). The caller also declines what
    /// [`touches_protected`] finds, and lists what was declined for the report.
    Unattended,
}

/// Decides who answers `request`.
pub fn route(request: &ApprovalRequest, access: &Access, mode: ApprovalMode) -> Route {
    match mode {
        ApprovalMode::DeclineAll => Route::Deny,
        ApprovalMode::Delegated => route_delegated(request, access),
        ApprovalMode::Unattended => match &request.command {
            Some(command) if is_outward(command) => Route::Deny,
            _ => Route::Allow,
        },
    }
}

/// Programs that delete or move files, judged by the paths they are given.
const REMOVERS: &[&str] = &["rm", "rmdir", "unlink", "mv", "shred", "trash", "truncate"];

/// Git commands that change a checkout's files (or its index and stash).
const GIT_TREE_CHANGERS: &[&str] = &[
    "add",
    "am",
    "apply",
    "checkout",
    "cherry-pick",
    "clean",
    "commit",
    "merge",
    "mv",
    "pull",
    "read-tree",
    "rebase",
    "reset",
    "restore",
    "revert",
    "rm",
    "stage",
    "stash",
    "switch",
    "update-index",
];

/// Whether `request` would change files under one of `protected` (the user's own checkout,
/// outside the run's worktrees): a file edit there, a git command that changes a checkout
/// run there (`-C`, a `cd` before it, or the request's folder), or a file remover given a path
/// there. Best effort over the command line, for an overnight run's never-list (PLAN.md
/// §10.8): a script or interpreter that does the same is not seen.
pub fn touches_protected(request: &ApprovalRequest, protected: &[std::path::PathBuf]) -> bool {
    use std::path::{Path, PathBuf};
    let roots: Vec<PathBuf> = protected
        .iter()
        .filter_map(|root| real_path(root))
        .collect();
    if roots.is_empty() {
        return false;
    }
    let cwd = request.cwd.as_deref().map(PathBuf::from);
    let resolve = |word: &str, base: Option<&Path>| -> Option<PathBuf> {
        let path = match word.strip_prefix("~/") {
            Some(rest) => PathBuf::from(std::env::var_os("HOME")?).join(rest),
            None => PathBuf::from(word),
        };
        let path = if path.is_absolute() {
            path
        } else {
            base?.join(path)
        };
        real_path(&lexically_normal(&path))
    };
    let inside = |path: Option<PathBuf>| {
        path.is_some_and(|path| roots.iter().any(|root| path.starts_with(root)))
    };
    // Removing or moving a folder that holds the checkout changes it too.
    let inside_or_holds = |path: Option<PathBuf>| {
        path.is_some_and(|path| {
            roots
                .iter()
                .any(|root| path.starts_with(root) || root.starts_with(&path))
        })
    };
    if request.kind == ApprovalKind::FileChange {
        return request
            .paths
            .iter()
            .any(|path| inside(resolve(path, cwd.as_deref())));
    }
    let Some(command) = &request.command else {
        return false;
    };
    // Where commands run: the request's folder, then each `cd` in the line, in order.
    let mut dir = cwd.clone();
    for words in simple_commands(command) {
        let words = strip_prefixes(&words);
        let Some((program, args)) = words.split_first() else {
            continue;
        };
        let name = program.rsplit('/').next().unwrap_or(program);
        match name {
            "cd" | "pushd" => {
                if let Some(target) = args.iter().find(|arg| !arg.starts_with('-')) {
                    dir = resolve(target, dir.as_deref()).or(dir);
                }
            }
            "git" => {
                let Some(at) = split_git_globals(args).command else {
                    continue;
                };
                let mut here = dir.clone();
                let mut globals = args[..at].iter();
                while let Some(arg) = globals.next() {
                    if arg == "-C"
                        && let Some(path) = globals.next()
                    {
                        here = resolve(path, here.as_deref());
                    }
                }
                if GIT_TREE_CHANGERS.contains(&args[at].as_str()) && inside(here) {
                    return true;
                }
            }
            name if REMOVERS.contains(&name) => {
                let in_dir = inside(dir.clone());
                let operands: Vec<&String> =
                    args.iter().filter(|arg| !arg.starts_with('-')).collect();
                for (n, arg) in operands.iter().enumerate() {
                    // `mv`'s last operand is where things go: a folder holding the checkout
                    // may receive them.
                    let into = name == "mv" && operands.len() > 1 && n + 1 == operands.len();
                    let path = resolve(arg, dir.as_deref());
                    let hit = if into {
                        inside(path)
                    } else {
                        inside_or_holds(path)
                    };
                    if hit || (in_dir && !arg.starts_with('/')) {
                        return true;
                    }
                }
            }
            _ => {}
        }
    }
    false
}

/// `path` with `.` and `..` folded away, without touching the file system.
fn lexically_normal(path: &std::path::Path) -> std::path::PathBuf {
    use std::path::Component;
    let mut normal = std::path::PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                normal.pop();
            }
            part => normal.push(part),
        }
    }
    normal
}

/// [`route`] under Approve for me.
fn route_delegated(request: &ApprovalRequest, access: &Access) -> Route {
    if let Some(command) = &request.command
        && is_outward(command)
    {
        return Route::AskUser;
    }
    if request.escalation || request.kind == ApprovalKind::Permissions {
        return Route::AskUser;
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

/// Whether a shell command line runs any [`ALWAYS_ASK`] command. Errs on the side of asking:
/// every simple command in the line is checked, including those inside `sh -c '…'` wrappers,
/// subshells and command substitutions.
pub fn is_outward(command: &str) -> bool {
    simple_commands(command).iter().any(|words| {
        ALWAYS_ASK
            .iter()
            .any(|pattern| matches_pattern(words, pattern))
    })
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

fn matches_pattern(words: &[String], pattern: &[&str]) -> bool {
    let words = strip_prefixes(words);
    let Some((program, rest)) = words.split_first() else {
        return false;
    };
    let name = program.rsplit('/').next().unwrap_or(program);
    if name != pattern[0] {
        return false;
    }
    let mut remaining = rest.iter();
    pattern[1..]
        .iter()
        .all(|wanted| remaining.any(|word| word == wanted))
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
    let mut chars = line.chars().peekable();

    let finish_word = |word: &mut String, in_word: &mut bool, current: &mut Vec<String>| {
        if *in_word {
            current.push(std::mem::take(word));
            *in_word = false;
        }
    };

    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                for c in chars.by_ref() {
                    if c == '\'' {
                        break;
                    }
                    word.push(c);
                }
            }
            '"' => {
                in_word = true;
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
            ';' | '&' | '|' | '\n' => {
                finish_word(&mut word, &mut in_word, &mut current);
                if !current.is_empty() {
                    local.push(std::mem::take(&mut current));
                }
            }
            c if c.is_whitespace() => finish_word(&mut word, &mut in_word, &mut current),
            c => {
                in_word = true;
                word.push(c);
            }
        }
    }
    finish_word(&mut word, &mut in_word, &mut current);
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

// ----- the command gate: argv-level matching -----------------------------------------------

/// The environment variable that carries a CLI session's gate grant to the command gate.
/// Codex's default environment filter drops names containing KEY, SECRET or TOKEN, so this one
/// avoids them.
pub const GATE_ENV: &str = "BRIGADIER_GATE";

/// Programs named in [`ALWAYS_ASK`], each once, in order. The command gate shims each of them
/// on a worker's PATH.
pub fn gate_programs() -> Vec<&'static str> {
    let mut programs: Vec<&'static str> = Vec::new();
    for words in ALWAYS_ASK {
        if !programs.contains(&words[0]) {
            programs.push(words[0]);
        }
    }
    programs
}

/// What the command gate does with a command line, judged from its argv alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArgvVerdict {
    /// Stays on the machine: runs without asking.
    Local,
    /// Affects the outside world (or cannot be judged): the user decides.
    Outward,
    /// A git subcommand that is not a git command, so it may be an alias. Look up
    /// `alias.<name>` with the real git and these global options, then judge
    /// [`expand_git_alias`]'s result.
    GitAlias { globals: Vec<String>, name: String },
}

/// Judges `argv` (`argv[0]` is the program, as invoked) for the command gate.
///
/// - git: global options before the subcommand are skipped (`git -C repo push`,
///   `git -c k=v --git-dir=… push`), the subcommand is matched exactly (so `git log --grep push`
///   runs), commands that run other commands (`submodule foreach`, `rebase --exec`,
///   `bisect run`) are judged by what they run, and anything that is not a git command may be
///   an alias ([`ArgvVerdict::GitAlias`]). An unknown global option asks.
/// - gh: an unknown top-level command may be an alias or an extension, so it asks.
/// - everything else: [`ALWAYS_ASK`] matching over the words.
pub fn classify_argv(argv: &[String]) -> ArgvVerdict {
    let Some((program, args)) = argv.split_first() else {
        return ArgvVerdict::Local;
    };
    let name = program.rsplit('/').next().unwrap_or(program);
    match name {
        "git" => classify_git(args),
        "gh" if args.first().is_some_and(|command| {
            !command.starts_with('-') && !GH_COMMANDS.contains(&command.as_str())
        }) =>
        {
            ArgvVerdict::Outward
        }
        _ => {
            let mut words = Vec::with_capacity(argv.len());
            words.push(name.to_owned());
            words.extend(args.iter().cloned());
            if ALWAYS_ASK
                .iter()
                .any(|pattern| matches_pattern(&words, pattern))
            {
                ArgvVerdict::Outward
            } else {
                ArgvVerdict::Local
            }
        }
    }
}

/// The command line a git alias stands for: `argv` with the alias word replaced by the
/// alias's words (git's own quoting rules). `None` for a shell alias (`!…`), which runs an
/// arbitrary command line: the gate asks for those.
pub fn expand_git_alias(argv: &[String], value: &str) -> Option<Vec<String>> {
    let value = value.trim();
    if value.starts_with('!') {
        return None;
    }
    let (program, args) = argv.split_first()?;
    let split = split_git_globals(args);
    let command = split.command?;
    let mut expanded = Vec::with_capacity(argv.len() + 4);
    expanded.push(program.clone());
    expanded.extend(args[..command].iter().cloned());
    expanded.extend(split_git_cmdline(value));
    expanded.extend(args[command + 1..].iter().cloned());
    Some(expanded)
}

/// Top-level `gh` commands (2.101) and help topics. Anything else is an alias or an
/// extension.
#[rustfmt::skip]
const GH_COMMANDS: &[&str] = &[
    "accessibility", "actions", "agent-task", "alias", "api", "attestation", "auth", "browse",
    "cache", "codespace", "completion", "config", "copilot", "discussion", "environment",
    "exit-codes", "extension", "formatting", "gist", "gpg-key", "help", "issue", "label",
    "licenses", "mintty", "org", "pr", "preview", "project", "reference", "release", "repo",
    "ruleset", "run", "search", "secret", "skill", "ssh-key", "status", "telemetry", "variable",
    "version", "workflow",
];

/// git's commands (built-ins and the scripts git ships). git never lets an alias shadow a
/// command, so these skip the alias lookup; any other word is looked up.
#[rustfmt::skip]
const GIT_COMMANDS: &[&str] = &[
    "add", "am", "annotate", "apply", "archimport", "archive", "backfill", "bisect", "blame",
    "branch", "bugreport", "bundle", "cat-file", "check-attr", "check-ignore", "check-mailmap",
    "check-ref-format", "checkout", "checkout-index", "cherry", "cherry-pick", "citool", "clean",
    "clone", "column", "commit", "commit-graph", "commit-tree", "config", "count-objects",
    "credential", "credential-cache", "credential-osxkeychain", "credential-store",
    "cvsexportcommit", "cvsimport", "cvsserver", "daemon", "describe", "diagnose", "diff",
    "diff-files", "diff-index", "diff-pairs", "diff-tree", "difftool", "fast-export", "fast-import",
    "fetch", "fetch-pack", "filter-branch", "fmt-merge-msg", "for-each-ref", "for-each-repo",
    "format-patch", "fsck", "fsck-objects", "fsmonitor--daemon", "gc", "get-tar-commit-id", "grep",
    "gui", "hash-object", "help", "hook", "http-backend", "http-fetch", "http-push", "imap-send",
    "index-pack", "init", "init-db", "instaweb", "interpret-trailers", "last-modified", "log",
    "ls-files", "ls-remote", "ls-tree", "mailinfo", "mailsplit", "maintenance", "merge",
    "merge-base", "merge-file", "merge-index", "merge-octopus", "merge-one-file", "merge-ours",
    "merge-recursive", "merge-resolve", "merge-subtree", "merge-tree", "mergetool", "mktag",
    "mktree", "multi-pack-index", "mv", "name-rev", "notes", "p4", "pack-objects", "pack-redundant",
    "pack-refs", "patch-id", "prune", "prune-packed", "pull", "push", "quiltimport", "range-diff",
    "read-tree", "rebase", "receive-pack", "reflog", "refs", "remote", "remote-ext", "remote-fd",
    "remote-ftp", "remote-ftps", "remote-http", "remote-https", "repack", "replace", "replay",
    "repo", "request-pull", "rerere", "reset", "restore", "rev-list", "rev-parse", "revert", "rm",
    "send-email", "send-pack", "sh-i18n--envsubst", "shell", "shortlog", "show", "show-branch",
    "show-index", "show-ref", "sparse-checkout", "stage", "stash", "status", "stripspace",
    "submodule", "subtree", "svn", "switch", "symbolic-ref", "tag", "unpack-file", "unpack-objects",
    "update-index", "update-ref", "update-server-info", "upload-archive", "upload-pack", "var",
    "verify-commit", "verify-pack", "verify-tag", "version", "web--browse", "whatchanged",
    "worktree", "write-tree",
];

/// Where git's global options end.
struct GitSplit {
    /// Index (in the arguments after `git`) of the subcommand; `None` when there is none.
    command: Option<usize>,
    /// A global option this list does not know, so its arguments cannot be told apart.
    unknown_option: bool,
}

/// Skips git's global options (git 2.54 `handle_options`).
fn split_git_globals(args: &[String]) -> GitSplit {
    const WITH_VALUE: &[&str] = &[
        "-C",
        "-c",
        "--git-dir",
        "--work-tree",
        "--namespace",
        "--super-prefix",
        "--config-env",
        "--attr-source",
        "--shallow-file",
        "--exec-path",
    ];
    const FLAGS: &[&str] = &[
        "-p",
        "--paginate",
        "-P",
        "--no-pager",
        "--no-replace-objects",
        "--no-lazy-fetch",
        "--no-optional-locks",
        "--no-advice",
        "--bare",
        "--literal-pathspecs",
        "--no-literal-pathspecs",
        "--glob-pathspecs",
        "--noglob-pathspecs",
        "--icase-pathspecs",
    ];
    // Print something and exit, or turn into `git help` / `git version`.
    const TERMINAL: &[&str] = &[
        "-h",
        "--help",
        "-v",
        "--version",
        "--html-path",
        "--man-path",
        "--info-path",
    ];
    let mut index = 0;
    while let Some(arg) = args.get(index) {
        let arg = arg.as_str();
        if !arg.starts_with('-') {
            return GitSplit {
                command: Some(index),
                unknown_option: false,
            };
        }
        let joined = arg
            .split_once('=')
            .is_some_and(|(name, _)| WITH_VALUE.contains(&name) || name == "--list-cmds");
        if TERMINAL.contains(&arg) || arg.starts_with("--list-cmds=") || arg == "--exec-path" {
            // `--exec-path` without a value prints the path.
            return GitSplit {
                command: None,
                unknown_option: false,
            };
        }
        if joined || FLAGS.contains(&arg) {
            index += 1;
        } else if WITH_VALUE.contains(&arg) {
            index += 2;
        } else {
            return GitSplit {
                command: None,
                unknown_option: true,
            };
        }
    }
    GitSplit {
        command: None,
        unknown_option: false,
    }
}

fn classify_git(args: &[String]) -> ArgvVerdict {
    let split = split_git_globals(args);
    if split.unknown_option {
        return ArgvVerdict::Outward;
    }
    let Some(command) = split.command else {
        return ArgvVerdict::Local;
    };
    let sub = args[command].as_str();
    let rest = &args[command + 1..];
    let outward_sub = ALWAYS_ASK.iter().any(|pattern| {
        pattern[0] == "git"
            && pattern
                .get(1)
                .is_some_and(|wanted| wanted.eq_ignore_ascii_case(sub))
            && pattern[2..]
                .iter()
                .all(|wanted| rest.iter().any(|word| word == wanted))
    });
    if outward_sub || git_runs_outward(sub, rest) {
        return ArgvVerdict::Outward;
    }
    if GIT_COMMANDS.contains(&sub) {
        ArgvVerdict::Local
    } else {
        ArgvVerdict::GitAlias {
            globals: args[..command].to_vec(),
            name: sub.to_owned(),
        }
    }
}

/// git commands that run a command line of their own.
fn git_runs_outward(sub: &str, rest: &[String]) -> bool {
    let command_line = match sub {
        // `git submodule [--quiet] foreach [--recursive] <command>`
        "submodule" => match rest.iter().position(|word| word == "foreach") {
            Some(at) => rest[at + 1..]
                .iter()
                .skip_while(|word| word.starts_with('-'))
                .cloned()
                .collect::<Vec<_>>()
                .join(" "),
            None => return false,
        },
        // `git rebase -x <cmd>`, `--exec <cmd>`, `--exec=<cmd>`
        "rebase" => {
            let mut commands = Vec::new();
            let mut words = rest.iter();
            while let Some(word) = words.next() {
                if word == "-x" || word == "--exec" {
                    commands.extend(words.next().cloned());
                } else if let Some(command) = word.strip_prefix("--exec=") {
                    commands.push(command.to_owned());
                } else if let Some(command) = word.strip_prefix("-x")
                    && !command.is_empty()
                {
                    commands.push(command.to_owned());
                }
            }
            commands.join("\n")
        }
        // `git bisect run <cmd> [<args>…]`
        "bisect" if rest.first().is_some_and(|word| word == "run") => rest[1..].join(" "),
        _ => return false,
    };
    is_outward(&command_line)
}

/// Splits an alias value the way git's `split_cmdline` does: whitespace separates words,
/// single and double quotes group them, and a backslash escapes the next character (outside
/// single quotes).
fn split_git_cmdline(value: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some('\''), '\'') | (Some('"'), '"') => quote = None,
            (Some('"') | None, '\\') => {
                in_word = true;
                if let Some(next) = chars.next() {
                    word.push(next);
                }
            }
            (None, '\'' | '"') => {
                in_word = true;
                quote = Some(c);
            }
            (None, c) if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            (_, c) => {
                in_word = true;
                word.push(c);
            }
        }
    }
    if in_word {
        words.push(word);
    }
    words
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
    fn an_unattended_run_approves_all_but_outward_actions() {
        let full = Access::Full;
        let sandboxed = scoped(std::env::temp_dir());
        for access in [&full, &sandboxed] {
            let unattended =
                |request: &ApprovalRequest| route(request, access, ApprovalMode::Unattended);
            // Leaving the sandbox, widening it, writing elsewhere: approved for the user.
            assert_eq!(
                unattended(&command("pnpm install --frozen-lockfile", None, true)),
                Route::Allow
            );
            let widen = ApprovalRequest {
                kind: ApprovalKind::Permissions,
                ..command("", None, true)
            };
            assert_eq!(unattended(&widen), Route::Allow);
            assert_eq!(
                unattended(&write(Path::new("/elsewhere/x.md"))),
                Route::Allow
            );
            // The never-list, however the command is spelled.
            for line in [
                "git push origin main",
                "/usr/bin/git push origin main",
                "env FOO=1 git -C repo push",
                "sh -c 'git push'",
                "bash -lc \"cd x && npm publish\"",
                "echo $(gh pr create --fill)",
                "nohup cargo publish",
                "/opt/homebrew/bin/gh release create v1",
            ] {
                assert_eq!(
                    unattended(&command(line, None, false)),
                    Route::Deny,
                    "{line}"
                );
                assert_eq!(
                    unattended(&command(line, None, true)),
                    Route::Deny,
                    "{line}"
                );
            }
        }
    }

    #[test]
    fn changes_to_the_users_checkout_are_found() {
        let place = Linked::new("protected");
        let repo = place.real().join("repo");
        let worktree = place.real().join("worktree");
        std::fs::create_dir_all(repo.join("src")).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();
        let protected = [repo.clone()];
        let repo_text = repo.display().to_string();
        let linked_repo = place.link().join("repo").display().to_string();
        let touches = |line: &str, cwd: &Path| {
            touches_protected(&command(line, Some(cwd), false), &protected)
        };
        for line in [
            format!("git -C {repo_text} reset --hard"),
            format!("git -C {linked_repo} checkout -- ."),
            format!("cd {repo_text} && git clean -fdx"),
            format!("sh -c 'cd {repo_text}; git stash'"),
            format!("/usr/bin/git -C {repo_text} commit -am x"),
            format!("rm -rf {repo_text}/src"),
            format!("rm -rf {linked_repo}"),
            format!("cd {repo_text}/src && rm -f main.rs"),
            format!("mv {repo_text}/src /tmp/elsewhere"),
            format!("git -C {}/../repo restore .", worktree.display()),
            format!("git -C {repo_text} add -A"),
            format!("git -C {repo_text} update-index --assume-unchanged x"),
            // A folder that holds the checkout.
            format!("rm -rf {}", place.real().display()),
            "rm -rf ..".to_owned(),
            format!("mv {} /tmp/elsewhere", place.real().display()),
        ] {
            assert!(touches(&line, &worktree), "{line}");
        }
        // Moving something into a folder that holds the checkout leaves the checkout alone.
        assert!(!touches(
            &format!("mv notes.md {}", place.real().display()),
            &worktree
        ));
        // In the request's folder.
        assert!(touches("git reset --hard", &repo));
        assert!(touches("rm -rf src", &repo.join("src")));
        for line in [
            "git reset --hard".to_owned(),
            "rm -rf target".to_owned(),
            format!("git -C {repo_text} status"),
            format!("git -C {repo_text} log --oneline"),
            format!("git -C {repo_text} diff"),
            format!("cat {repo_text}/src/main.rs"),
            format!("cp {repo_text}/src/main.rs ."),
            "rm -rf /tmp/brigadier-test-1234".to_owned(),
        ] {
            assert!(!touches(&line, &worktree), "{line}");
        }
        assert!(touches_protected(
            &write(&repo.join("src/new.rs")),
            &protected
        ));
        assert!(touches_protected(
            &write(&place.link().join("repo/README.md")),
            &protected
        ));
        assert!(!touches_protected(
            &write(&worktree.join("README.md")),
            &protected
        ));
        assert!(!touches_protected(&write(&repo.join("x")), &[]));
    }
}
