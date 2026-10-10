//! Branch names as git allows them (`git check-ref-format --branch`), and the prefix the user
//! puts before the names Brigadier makes up.

/// Why `name` can't be a branch's name, in plain words, or `None` when git takes it. The
/// rules of `git check-ref-format --branch`: no part that starts with `.` or ends with
/// `.lock`, no `..`, `@{`, `//` or `\`, no control character, space, `~`, `^`, `:`, `?`, `*`
/// or `[`, not starting with `-` or `/`, not ending with `/` or `.`, and not `@` alone.
pub fn branch_name_problem(name: &str) -> Option<String> {
    if name.is_empty() {
        return Some("A branch name can't be empty.".into());
    }
    if name == "@" {
        return Some("A branch can't be named \"@\".".into());
    }
    if name.starts_with('-') {
        return Some("A branch name can't start with \"-\".".into());
    }
    if name.starts_with('/') || name.ends_with('/') {
        return Some("A branch name can't start or end with \"/\".".into());
    }
    if name.ends_with('.') {
        return Some("A branch name can't end with \".\".".into());
    }
    for (bad, what) in [
        ("..", "\"..\""),
        ("@{", "\"@{\""),
        ("//", "\"//\""),
        ("\\", "\"\\\""),
    ] {
        if name.contains(bad) {
            return Some(format!("A branch name can't contain {what}."));
        }
    }
    if let Some(c) = name
        .chars()
        .find(|c| c.is_ascii_control() || matches!(c, ' ' | '~' | '^' | ':' | '?' | '*' | '['))
    {
        return Some(if c == ' ' {
            "A branch name can't contain spaces.".into()
        } else if c.is_ascii_control() {
            "A branch name can't contain control characters.".into()
        } else {
            format!("A branch name can't contain \"{c}\".")
        });
    }
    for part in name.split('/') {
        if part.starts_with('.') {
            return Some("No part of a branch name can start with \".\".".into());
        }
        if part.ends_with(".lock") {
            return Some("No part of a branch name can end with \".lock\".".into());
        }
    }
    None
}

/// Why `prefix` can't go before the branch names Brigadier makes up, or `None`. It is checked
/// in the names it would make: a session's, a worker's and a suggested one. Empty is fine
/// (no prefix); a trailing `/` is what separates it.
pub fn branch_prefix_problem(prefix: &str) -> Option<String> {
    if prefix.is_empty() {
        return None;
    }
    if prefix.contains("//") {
        return Some("The prefix can't contain \"//\".".into());
    }
    if prefix.ends_with('.') {
        return Some("The prefix can't end with \".\".".into());
    }
    [
        "0a1b2c3d/session",
        "0a1b2c3d/task-1-fix-the-login",
        "fix-the-login",
    ]
    .iter()
    .find_map(|rest| branch_name_problem(&format!("{prefix}{rest}")))
    .map(|problem| problem.replace("A branch name", "The prefix"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rules agree with git's own check on names that test each one. (`@` alone it reads
    /// as the current branch, which `git branch @` refuses to create.)
    #[test]
    fn branch_names_follow_git_check_ref_format() {
        let names = [
            "brigadier/abc/session",
            "feature/x",
            "x",
            "-x",
            "/x",
            "x/",
            "x.",
            "a..b",
            "a@{b",
            "a//b",
            "a\\b",
            "a b",
            "a~b",
            "a^b",
            "a:b",
            "a?b",
            "a*b",
            "a[b",
            "a\tb",
            "a\u{7f}b",
            ".a",
            "a/.b",
            "a.lock",
            "a.lock/b",
            "a/b.lock",
            "a.locks",
            "a@b",
            "a.b",
            "ü/ä",
        ];
        for name in names {
            let git = std::process::Command::new("git")
                .args(["check-ref-format", "--branch", name])
                .output()
                .expect("git runs");
            assert_eq!(
                branch_name_problem(name).is_none(),
                git.status.success(),
                "{name:?}: {:?}",
                branch_name_problem(name)
            );
        }
    }

    #[test]
    fn at_alone_is_no_branch_name() {
        assert!(branch_name_problem("@").is_some());
    }

    #[test]
    fn a_prefix_is_checked_in_the_names_it_makes() {
        for good in ["", "brigadier/", "sg-", "team/sg/", "a.b/"] {
            assert_eq!(branch_prefix_problem(good), None, "{good:?}");
        }
        for bad in [
            "a..b/", "a b/", "x.lock/", ".x/", "-x/", "/x/", "a//", "x.", "a@{", "a~/", "a:/",
        ] {
            assert!(branch_prefix_problem(bad).is_some(), "{bad:?}");
        }
        assert_eq!(
            branch_prefix_problem("a b/").as_deref(),
            Some("The prefix can't contain spaces.")
        );
    }
}
