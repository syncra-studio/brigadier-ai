//! Which of a worker's commands are heavy: builds and test runs, the work that heats the
//! machine. Brigadier never sees a CLI's shell commands as they are asked for; it finds them
//! in the CLI's process tree by what they run.
//!
//! A command counts when its program is a build tool or test runner (`cargo build`, `go test`,
//! `xcodebuild`, `make`, `pytest`, `tsc`, …), or a package manager running a build, test,
//! typecheck or lint script (`pnpm build`, `npm test`). Development servers and watchers
//! (`dev`, `watch`, `serve`, `start`) don't: they run for as long as the worker wants them.
//! The topmost such process of a tree is the command; what it starts (rustc, the test
//! binaries) belongs to it.

/// Build tools and test runners that are heavy whatever they are asked.
const TOOLS: &[&str] = &[
    "rustc",
    "swiftc",
    "xcodebuild",
    "gradle",
    "gradlew",
    "mvn",
    "mvnw",
    "make",
    "gmake",
    "ninja",
    "bazel",
    "bazelisk",
    "buck2",
    "msbuild",
    "clang",
    "clang++",
    "gcc",
    "g++",
    "cc",
    "c++",
    "pytest",
    "tsc",
    "jest",
    "vitest",
    "playwright",
    "webpack",
    "rollup",
    "turbo",
    "nx",
];

/// Tools heavy only for these subcommands.
const SUBCOMMANDS: &[(&str, &[&str])] = &[
    (
        "cargo",
        &[
            "build", "b", "test", "t", "check", "c", "clippy", "bench", "doc", "nextest",
            "install", "run", "r", "zigbuild", "llvm-cov", "miri",
        ],
    ),
    (
        "go",
        &["build", "test", "vet", "install", "run", "generate"],
    ),
    ("swift", &["build", "test", "run"]),
    ("dotnet", &["build", "test", "publish", "run"]),
    ("cmake", &["--build"]),
];

/// Package managers: heavy when they run one of [`SCRIPTS`].
const PACKAGE_MANAGERS: &[&str] = &["npm", "pnpm", "yarn", "bun", "npx", "pnpx", "bunx"];

/// Script names (or words in them) that build or test.
const SCRIPTS: &[&str] = &[
    "build",
    "test",
    "tests",
    "typecheck",
    "tsc",
    "lint",
    "check",
    "vitest",
    "jest",
    "playwright",
    "e2e",
];

/// Words that make any command a long-running server or watcher rather than a build.
const LONG_RUNNING: &[&str] = &[
    "dev", "watch", "--watch", "serve", "--serve", "start", "preview",
];

/// Tools whose `-w` means watch (for a compiler it silences warnings; for pnpm it means the
/// workspace root).
const WATCH_W: &[&str] = &["tsc", "vitest", "jest", "webpack", "rollup"];

/// Runtimes with heavy subcommands of their own (`bun test`, `deno compile`); otherwise
/// they run a script, or (`bun run`) a package script.
const RUNTIMES: &[(&str, &[&str])] = &[
    ("bun", &["test", "build"]),
    ("deno", &["test", "bench", "compile", "check"]),
];

/// Global options that take a value as the next argument (`go -C dir test`, `cargo --color
/// always test`): skipped when looking for the subcommand.
const VALUE_OPTIONS: &[&str] = &[
    "-C",
    "--color",
    "--config",
    "-Z",
    "--manifest-path",
    "--package-path",
    "--chdir",
    "--target-dir",
];

/// Interpreters whose script names the command (`node …/vitest.mjs`, `python -m pytest`,
/// `sh ./gradlew build`). A shell given a command line (`bash -c "cargo test"`) is not the
/// command: what it starts is.
const INTERPRETERS: &[&str] = &[
    "node", "bun", "deno", "python", "python3", "sh", "bash", "zsh", "dash",
];

/// A program's name without its folder, extension (`.exe`, `.js`, …) or case.
fn program(arg: &str) -> String {
    let name = arg.rsplit(['/', '\\']).next().unwrap_or(arg);
    let name = name
        .strip_suffix(".exe")
        .or_else(|| name.strip_suffix(".cmd"))
        .or_else(|| name.strip_suffix(".mjs"))
        .or_else(|| name.strip_suffix(".cjs"))
        .or_else(|| name.strip_suffix(".js"))
        .unwrap_or(name);
    name.to_ascii_lowercase()
}

/// The first argument that isn't an option or an option's value, with its index.
fn subcommand(args: &[String]) -> Option<(usize, &String)> {
    let mut index = 0;
    while let Some(arg) = args.get(index) {
        if arg == "--build" || !arg.starts_with(['-', '+']) {
            return Some((index, arg));
        }
        index += if VALUE_OPTIONS.contains(&arg.as_str()) {
            2
        } else {
            1
        };
    }
    None
}

/// Whether a process running `argv` is a heavy command.
pub(crate) fn is_heavy(argv: &[String]) -> bool {
    let Some(first) = argv.first() else {
        return false;
    };
    let mut name = program(first);
    let mut rest = &argv[1..];
    let long_running = |name: &str, rest: &[String]| {
        rest.iter().any(|arg| {
            LONG_RUNNING.contains(&arg.as_str())
                || arg.ends_with(":dev")
                || arg.ends_with(":watch")
                || (arg == "-w" && WATCH_W.contains(&name))
        })
    };
    if let Some((_, own)) = RUNTIMES.iter().find(|(runtime, _)| *runtime == name) {
        match subcommand(rest) {
            Some((_, sub)) if own.contains(&sub.as_str()) => return !long_running(&name, rest),
            Some((index, sub)) if sub == "run" || sub == "x" => {
                // `bun run build` is a package script, as `pnpm build`.
                let script = &rest[index + 1..];
                return !long_running(&name, script) && runs_a_build_script(script);
            }
            _ => {}
        }
    }
    if INTERPRETERS.contains(&name.as_str()) {
        // The script (or `-m module`) is the program.
        let mut args = rest.iter().enumerate();
        let script = loop {
            match args.next() {
                Some((index, arg)) if arg == "-m" => {
                    break rest.get(index + 1).map(|module| (index + 1, module));
                }
                Some((_, arg)) if arg.starts_with('-') => continue,
                Some((index, arg)) => break Some((index, arg)),
                None => break None,
            }
        };
        let Some((index, script)) = script else {
            return false;
        };
        name = program(script);
        rest = &rest[index + 1..];
    }
    if long_running(&name, rest) {
        return false;
    }
    if TOOLS.contains(&name.as_str()) {
        return true;
    }
    if let Some((_, subcommands)) = SUBCOMMANDS.iter().find(|(tool, _)| *tool == name) {
        return subcommand(rest).is_some_and(|(_, sub)| subcommands.contains(&sub.as_str()));
    }
    if PACKAGE_MANAGERS.contains(&name.as_str()) {
        return runs_a_build_script(rest);
    }
    false
}

/// Whether a package manager's arguments name a build, test, typecheck or lint script.
fn runs_a_build_script(args: &[String]) -> bool {
    args.iter().any(|arg| {
        arg.split([':', '-', '_'])
            .any(|word| SCRIPTS.contains(&word))
    })
}

/// How a thread row names a command: its program and first few arguments, short.
pub(crate) fn label(argv: &[String]) -> String {
    const MAX: usize = 48;
    let mut words = Vec::new();
    let mut args = argv.iter();
    if let Some(first) = args.next() {
        let name = program(first);
        if INTERPRETERS.contains(&name.as_str()) {
            // `node …/vitest.mjs run` reads as `vitest run`.
            for arg in args.by_ref() {
                if !arg.starts_with('-') {
                    words.push(program(arg));
                    break;
                }
            }
        } else {
            words.push(name);
        }
    }
    words.extend(args.take(3).cloned());
    let mut text = words.join(" ");
    if text.len() > MAX {
        let mut end = MAX;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push('…');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn builds_and_tests_are_heavy() {
        for line in [
            "cargo test -p brigadier-core",
            "/Users/x/.cargo/bin/cargo clippy --workspace --all-targets",
            "cargo +nightly build",
            "go test ./...",
            "xcodebuild -scheme App build",
            "make -j8",
            "pnpm build",
            "pnpm -C apps/desktop typecheck",
            "npm run test",
            "yarn test:unit",
            "npx vitest run",
            "node /repo/node_modules/vitest/vitest.mjs run",
            "python3 -m pytest tests",
            "/usr/bin/swift build",
            "cmake --build build",
            "tsc --noEmit",
            "C:\\tools\\cargo.exe build",
            "go -C /tmp/repo test ./...",
            "cargo --color always test",
            "cargo -Z unstable-options build",
            "clang -w -c main.c",
            "bun test",
            "bun build ./index.ts",
            "bun run build",
            "deno test",
            "pnpm -w build",
        ] {
            assert!(is_heavy(&argv(line)), "{line}");
        }
    }

    #[test]
    fn servers_watchers_and_everything_else_are_not() {
        for line in [
            "cargo fmt --all",
            "cargo metadata",
            "cargo watch -x test",
            "pnpm dev",
            "tsc -w",
            "bun run dev",
            "bun index.ts",
            "deno run main.ts",
            "pnpm install",
            "pnpm run build --watch",
            "npm run start",
            "yarn test:watch",
            "vite",
            "node /usr/local/lib/node_modules/@anthropic-ai/claude-code/cli.js",
            "codex app-server",
            "git status",
            "rg build",
            "python3 script.py",
            "brigadierd mcp",
            "",
        ] {
            assert!(!is_heavy(&argv(line)), "{line}");
        }
    }

    #[test]
    fn a_shell_is_heavy_only_for_a_heavy_script() {
        let bash = |args: &[&str]| args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>();
        assert!(!is_heavy(&bash(&["/bin/bash", "-c", "cargo test"])));
        assert!(!is_heavy(&bash(&[
            "/bin/zsh",
            "-l",
            "-c",
            "pnpm build && pnpm test"
        ])));
        assert!(is_heavy(&bash(&["/bin/sh", "./gradlew", "build"])));
        assert!(is_heavy(&bash(&["/bin/sh", "/tmp/bin/cargo", "test"])));
        assert!(!is_heavy(&bash(&["/bin/sh", "./setup.sh"])));
    }

    #[test]
    fn labels_are_short_and_name_the_tool() {
        assert_eq!(
            label(&argv(
                "/Users/x/.cargo/bin/cargo test -p brigadier-core --lib"
            )),
            "cargo test -p brigadier-core"
        );
        assert_eq!(
            label(&argv("node /repo/node_modules/vitest/vitest.mjs run")),
            "vitest run"
        );
        let long = label(&argv(
            "cargo test --package a-very-long-package-name-indeed-it-is --features everything",
        ));
        assert!(
            long.ends_with('…') && long.len() <= 48 + '…'.len_utf8(),
            "{long}"
        );
    }
}
