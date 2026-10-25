//! The `/brigade` skill (S15): how a session runs workers of its own through `brigadier`. It is
//! installed for every session and Chat, on both CLIs: a Claude session loads it as a plugin,
//! and Codex finds it in its user skills folder (`P/codex`). A user copy at
//! `~/.brigadier/brigade/SKILL.md` overrides the shipped one and survives updates; Settings
//! creates it from the shipped one, and resets it.

use std::path::{Path, PathBuf};

use super::SessionManager;
use crate::{Error, Result};

/// The skill as shipped.
pub(crate) const SKILL: &str = include_str!("brigade/SKILL.md");

/// The `/fusion` skill (S36), shipped beside it in the same plugin: one consult per CLI when
/// the user is uncertain, then one merged answer.
pub(crate) const FUSION: &str = include_str!("fusion/SKILL.md");

/// Whether the user's copy overrides the shipped skill, and where it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrigadeSkill {
    pub custom: bool,
    pub path: PathBuf,
}

impl SessionManager {
    /// The user's copy of the skill.
    pub(super) fn brigade_override_path(&self) -> PathBuf {
        if cfg!(test) {
            return self.data_dir.join("brigade-user/SKILL.md");
        }
        std::env::home_dir()
            .unwrap_or_else(|| self.data_dir.clone())
            .join(".brigadier/brigade/SKILL.md")
    }

    /// The skill's plugin, written with the user's copy when there is one, else the shipped
    /// one; `None` when it can't be written.
    pub(super) fn brigade_plugin(&self) -> Option<PathBuf> {
        let text = user_copy(&self.brigade_override_path());
        match write_plugin(
            &self.data_dir.join("brigade"),
            text.as_deref().unwrap_or(SKILL),
        ) {
            Ok(plugin) => Some(plugin),
            Err(error) => {
                tracing::warn!(%error, "couldn't write the brigade skill");
                None
            }
        }
    }

    /// Whether the user's copy is in use.
    pub fn brigade_skill(&self) -> BrigadeSkill {
        let path = self.brigade_override_path();
        BrigadeSkill {
            custom: user_copy(&path).is_some(),
            path,
        }
    }

    /// Writes the user's copy as the shipped skill, to edit, unless one is there: sessions
    /// started from now on get it.
    pub fn create_brigade_skill(&self) -> Result<BrigadeSkill> {
        let path = self.brigade_override_path();
        if user_copy(&path).is_none() {
            let written = path
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| std::fs::write(&path, SKILL));
            if let Err(error) = written {
                return Err(Error::Invalid(format!("{}: {error}", path.display())));
            }
            self.brigade_plugin();
        }
        Ok(self.brigade_skill())
    }

    /// Removes the user's copy: sessions started from now on get the shipped skill.
    pub fn reset_brigade_skill(&self) -> Result<BrigadeSkill> {
        let path = self.brigade_override_path();
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(Error::Invalid(format!("{}: {error}", path.display()))),
        }
        self.brigade_plugin();
        Ok(self.brigade_skill())
    }
}

/// The user's copy, unless it is missing or empty.
fn user_copy(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .filter(|text| !text.trim().is_empty())
}

/// Writes `<dir>/.claude-plugin/plugin.json`, `<dir>/skills/brigade/SKILL.md` and
/// `<dir>/skills/fusion/SKILL.md`.
fn write_plugin(dir: &Path, skill: &str) -> std::io::Result<PathBuf> {
    let manifest = serde_json::json!({
        "name": "brigade",
        "description": "How a Brigadier session runs workers of its own, and fuses models' advice.",
        "version": env!("CARGO_PKG_VERSION"),
    });
    write_if_changed(
        &dir.join(".claude-plugin/plugin.json"),
        &serde_json::to_string_pretty(&manifest).unwrap_or_default(),
    )?;
    write_if_changed(&dir.join("skills/brigade/SKILL.md"), skill)?;
    write_if_changed(&dir.join("skills/fusion/SKILL.md"), FUSION)?;
    Ok(dir.to_path_buf())
}

fn write_if_changed(path: &Path, contents: &str) -> std::io::Result<()> {
    if std::fs::read_to_string(path).is_ok_and(|current| current == contents) {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, contents)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_skill_is_short_and_says_what_it_is_for() {
        assert!(
            SKILL.lines().count() <= 150,
            "{} lines",
            SKILL.lines().count()
        );
        // A session that reads it keeps it in context for every later call (the w152 bench:
        // about 3.1k tokens written once, then read again on each call). The short-job path
        // added 0.7 KB to save a whole call (w155); keeping a watcher on what ends after the
        // turn, 0.27 KB (D21); grilling every new job with the CLI's own question tool,
        // 0.09 KB (harness D7, D28); the session's own subagents and quick checks, naked workers
        // and the review flow, 0.61 KB after tightening (harness D3, D8, D13, D22; proposed); the
        // question tool winning over another skill's text format, and a round too big for one
        // call on one card, 0.28 KB (harness D36).
        assert!(SKILL.len() <= 10_840, "{} bytes", SKILL.len());
        // Said once: the standing instructions already hold a worker's claim, and the Waves
        // list holds the events.
        assert!(!SKILL.contains("A worker's claim is not a fact"));
        assert_eq!(SKILL.matches("usage limit").count(), 1);
        // Waits in proportion (the w152 bench): a background wait and the turn it ends cost a
        // small job's parent about 9 cost units more than a foreground one.
        assert!(SKILL.contains("One short job (minutes) and nothing else to do: run `brigadier wait <id>` in the foreground; it returns with the event. Longer: run `brigadier wait` as a background command and end your turn"));
        // A short job's parent takes four calls, not five or more (the w152/w154 benches): the
        // reads in one, start and wait in one, its evidence in one, then the answer. It checks
        // evidence and builds nothing itself (run2-D24); a small edit it makes itself (run2-D43).
        for line in [
            "## One short job: four calls",
            "so skip `--help`",
            "`brigadier start \"<brief>\" --work \"<Work>\" … && brigadier wait`",
            "check its evidence (`git log`, the output it reported)",
            "(a small edit you make yourself)",
            "Close it unless related work is coming.",
        ] {
            assert!(SKILL.contains(line), "{line}");
        }
        assert!(SKILL.starts_with("---\nname: brigade\ndescription: "));
        for command in [
            "brigadier models",
            "brigadier start",
            "brigadier wait",
            "brigadier send",
        ] {
            assert!(SKILL.contains(command), "{command}");
        }
        // A Chat runs workers too (S28).
        assert!(!SKILL.contains("needs a project"));
        assert!(SKILL.contains("--repo"));
        // Something that ends after the turn keeps a watcher running, as advice (D21).
        assert!(SKILL.contains("`gh run watch <id> --exit-status`, or Monitor"));
        // An app the user asked to see is started by the session itself, as a preview.
        assert!(SKILL.contains("brigadier preview start"));
        // S22b's four lines: decisions travel to workers, derived documents and the user (a
        // gap asked with the CLI's own question tool now, harness D7).
        for line in [
            "The brief carries the rules that bear on its job, verbatim, with their ledger ids",
            "A plan or brief built from decisions cites their ids. Lines of your own say \"proposed\", and a gap the decisions don't cover goes to the user, never a silent fill.",
            "run `brigadier ledger check <file>` on it, and give its reviewer the ledger with the question \"what is missing, changed or added?\"",
            "When the user writes after a gap, raise the oldest open question first",
        ] {
            assert!(SKILL.contains(line), "{line}");
        }
        assert!(SKILL.contains("the fusion skill"));
        // Deciding (run2-D40): the session judges a message queued while it works.
        assert!(SKILL.contains("judge it soon with `brigadier queue <id>`"));
        // Reviews in proportion (the w123 bench: a two-file job tripled in time under them),
        // adversarial, on a routed reviewer (run2-D33, D35, D36): the big job's own plan before
        // workers (run2-D33), a worker's plan optionally (proposed), and one end verification
        // copied from the Delegator, on another ready provider than the author's (harness D4,
        // D22).
        for line in [
            "A big job (several parts, risky, or the user asked) gets two reviews; a small change with its tests passing lands without a review or a critique.",
            "**Plan review** of the big job's plan (`--reviewing session`) before it goes to workers",
            "**End verification**, once, on the integrated tree, never mid-run: a fresh reviewer code-reviews the job's base..head adversarially, checks every \"done when\" for real, fixes what it can, and reports pass or fail per item with evidence.",
            "`brigadier start --reviewing <id>|session`: it runs on another ready provider than the author's, else a fresh session of theirs, and reads `brigadier show <id>` and `brigadier context`.",
            "Its brief: break it",
            "note each rejected one with its reason",
        ] {
            assert!(SKILL.contains(line), "{line}");
        }
        assert!(!SKILL.contains("another vendor or model"));
        // The session orchestrates (run2-D23, harness D2): it does the talking, look-ups, small
        // edits, git and checking, with its own subagents (harness D3, superseding run2-D43) and
        // a quick check after a small edit (harness D8, amending run2-D24); workers do builds,
        // full suites and research, on the routing's rows (run2-D36), naked (D13, D17, D18).
        for line in [
            "You are an orchestrator.",
            "small edits (a few lines in a file or two); git, merges, pushes and CI; checking workers' evidence",
            "Your subagents may do look-ups, cross-file reads and small edits (the routing's models, effort at most high); after such an edit, one quick check (a test file, a type check). Full suites, app builds and live runs go to workers or CI.",
            "anything that needs a build or tests (the checks on your integrated tree too), and research beyond a quick look-up: a worker on the routing's search row",
            "[--mcp NAME]…",
            "MCP servers only by `--mcp` (`brigadier context mcps` lists them)",
            "it pulls decisions, skills and the rest with `brigadier context`",
            "The user's routing decides each worker's agent, model and effort.",
            "the same Work's next row the rules allow; never a model or effort the rows don't name",
        ] {
            assert!(SKILL.contains(line), "{line}");
        }
        assert!(!SKILL.contains("- Default: do it yourself."));
        assert!(!SKILL.contains("--lean"));
        assert!(!SKILL.contains("never build or test"));
        assert!(!SKILL.contains("have no subagents"));
        assert!(
            !SKILL.contains("codex review"),
            "reviews go through routing"
        );
        // Up to N workers at once, N from `brigadier models` (run2-D31).
        assert!(SKILL.contains("N is the \"Workers at once\" line of `brigadier models`"));
        // Which worker gets what, follow-ups and reuse (run2-D38, D42).
        for line in [
            "Give each part to the worker that owns its files or knows its area",
            "A follow-up from the user on a worker's area goes to that worker with `brigadier send`, not to a new one.",
            "A finished, unclosed worker takes related next work by `brigadier send`",
            "Close one once no related work is expected; a closed one can't resume",
        ] {
            assert!(SKILL.contains(line), "{line}");
        }
        // Grilling is part of the brigade (run2-D32): look-ups first, then every question it
        // can ask on one card, each with a recommendation, a next round only for what hangs on
        // those answers (the user: never one question at a time); the shared understanding
        // said back; workers' questions it answers itself when it can; what the user always
        // decides.
        assert!(!SKILL.contains("one question at a time"));
        for line in [
            "## Ask the user: grilling",
            "First answer all you can yourself (the code, the ledger, a web search, a research worker).",
            "Ask all you can in one call of your CLI's own question tool, in plain words: recommended option first, labelled \"… (Recommended)\", why in its description; a next round only for what hangs on those answers, never one call per question.",
            "`brigadier ask --question Q [--option O]… [--question …]` | One card: no question tool, or a worker's question (in the background). |",
            "Say the shared understanding back in a few lines, then start.",
            // Another skill's text format never wins over the card; a round bigger than one
            // call of the question tool goes on one `brigadier ask` card (harness D36).
            "More questions or options than one call takes: one `brigadier ask` card instead.",
            "Questions go in the question tool or on a card, never as text in your reply, even when another skill (a grilling or interview skill) shows a text format: it decides what to ask, never how it's shown.",
            "answer it yourself when the plan, the user's words, the code or a web search settle it; otherwise ask the user once",
            "Always ask the user, unless they already said so: spending money; a merge into the base branch, a push, publish or release; credentials or sign-ups; changing a decision they settled.",
        ] {
            assert!(SKILL.contains(line), "{line}");
        }
        // D7 as D28 reads it: every new job opens with a grilling round, skipped only when
        // nothing is left to decide; nothing small and clear starts unasked.
        for line in [
            "Every new job (build, change, fix, design, research) opens with a grilling round.",
            "Skip it only when nothing is left to decide: a pure question, or an exact one-line instruction.",
        ] {
            assert!(SKILL.contains(line), "{line}");
        }
        assert!(!SKILL.contains("A small, clear job starts at once"));
        assert!(!SKILL.contains("Before a big or unclear job"));
        // Where work lands (the user): workers merge into the session's branch, and all work
        // stays there, a New worktree's or a Local checkout's; at the end the session asks
        // before a push or a merge into the base branch, never landing on it by itself.
        assert!(SKILL.contains("merge your branch into mine"));
        assert!(SKILL.contains("All work stays on your branch; at the end ask the user whether to push, and in a New worktree whether to merge it into its base branch."));
        assert!(!SKILL.contains("land on what the user chose"));
        // The reply after a block of work (run2-D27).
        assert!(SKILL.contains("After a block of work, at most 3 to 5 short lines"));
    }

    /// Codex parses a skill's frontmatter as YAML and skips a skill it can't parse: a plain
    /// value with ": " or " #" in it is not YAML.
    #[test]
    fn both_skills_frontmatter_is_plain_yaml() {
        for skill in [SKILL, FUSION] {
            let head = skill.split("\n---\n").next().unwrap();
            for line in head.lines().skip(1) {
                let (_, value) = line.split_once(": ").unwrap();
                assert!(!value.contains(": ") && !value.contains(" #"), "{line}");
            }
        }
    }

    #[test]
    fn the_fusion_skill_consults_each_cli_through_its_own_row() {
        assert!(FUSION.starts_with("---\nname: fusion\ndescription: "));
        assert!(FUSION.lines().count() <= 150);
        for line in [
            "/fusion <question>",
            "`fusion-claude`, `fusion-codex`",
            // Every worker is naked (D13): no `--lean`.
            "brigadier start \"<brief>\" --work fusion-claude",
            "with nothing piped after it",
            "Answer only; change no file.",
            "One cross round.",
            "You judge; never count votes.",
            // Within the worker count, on the routing's rows, in the short reply (run2-D27,
            // D31, D36).
            "together while \"Workers at once\" (in `brigadier models`) allows, else one after another",
            "take the same Work's next row the rules allow; with none, skip that CLI and say so",
            "in 3 to 5 short lines with no headings",
        ] {
            assert!(FUSION.contains(line), "{line}");
        }
        assert!(!FUSION.contains("with `--model`"));
        assert!(!FUSION.contains("lean"));
    }
}
