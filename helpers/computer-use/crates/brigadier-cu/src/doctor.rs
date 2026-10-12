//! `brigadier-cu doctor` — environment + install diagnostic probes.
//!
//! The doctor subcommand runs a battery of platform-aware probes and emits
//! a structured report (plain text by default, JSON via `--json`). Each probe
//! produces one line tagged `[ok]`, `[warn]`, or `[err]` so the output is
//! grep-friendly without losing detail.
//!
//! Exit code: `0` when every probe is `[ok]` or `[warn]`. Non-zero only when
//! at least one `[err]` probe failed — e.g. the binary cannot read its own
//! install dir. Warnings (e.g. running outside an interactive desktop on
//! Windows) do not fail because they are sometimes the expected state (CI
//! invocations of `doctor` to render the report).
//!
//! ## Probe categories
//!
//! - **Cross-platform**: version + arch, install layout, home dir.
//! - **Windows**: interactive desktop session detection (Session 0 warning),
//!   UI Automation COM availability, top-level window enumeration count.
//! - **Linux**: `DISPLAY` / `WAYLAND_DISPLAY` presence, X11 connection
//!   reachability, AT-SPI bus availability hint.
//! - **macOS**: existing legacy-cleanup steps (LaunchAgent plist + update
//!   script), plus a hint to run `brigadier-cu diagnose` for a full
//!   TCC / cdhash / install layout dump.

use std::path::PathBuf;

/// Probe outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ok,
    Warn,
    Err,
}

impl Status {
    fn tag(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Warn => "warn",
            Status::Err => "err",
        }
    }
}

/// One probe result.
#[derive(Debug, Clone)]
pub struct Probe {
    /// Short stable label (e.g. `"binary"`, `"home dir"`).
    pub label: String,
    pub status: Status,
    /// One-line summary printed next to the tag.
    pub message: String,
    /// Optional multi-line detail (continuation lines indented under the
    /// summary in text mode, separate JSON field in `--json` mode).
    pub detail: Option<String>,
}

impl Probe {
    pub fn ok(label: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            status: Status::Ok,
            message: message.into(),
            detail: None,
        }
    }
    pub fn warn(label: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            status: Status::Warn,
            message: message.into(),
            detail: None,
        }
    }
    pub fn err(label: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            status: Status::Err,
            message: message.into(),
            detail: None,
        }
    }
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// Aggregated probe results for one `doctor` run.
#[derive(Debug, Clone, Default)]
pub struct Report {
    pub probes: Vec<Probe>,
}

impl Report {
    pub fn push(&mut self, probe: Probe) {
        self.probes.push(probe);
    }

    /// True iff at least one probe is `Status::Err`. Drives the process
    /// exit code — warnings never fail the run.
    pub fn has_errors(&self) -> bool {
        self.probes.iter().any(|p| p.status == Status::Err)
    }

    /// Plain-text rendering. Each probe is one summary line; multi-line
    /// `detail` blocks are indented underneath.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        for probe in &self.probes {
            out.push_str(&format!(
                "[{tag:<4}] {label}: {msg}\n",
                tag = probe.status.tag(),
                label = probe.label,
                msg = probe.message,
            ));
            if let Some(detail) = &probe.detail {
                for line in detail.lines() {
                    out.push_str("         ");
                    out.push_str(line);
                    out.push('\n');
                }
            }
        }
        out
    }

    /// JSON rendering: `{ "probes": [...], "ok": bool }`. Each probe is
    /// `{ "label", "status", "message", "detail" }`.
    pub fn to_json(&self) -> serde_json::Value {
        let probes: Vec<serde_json::Value> = self
            .probes
            .iter()
            .map(|p| {
                let mut obj = serde_json::Map::new();
                obj.insert("label".into(), serde_json::Value::String(p.label.clone()));
                obj.insert(
                    "status".into(),
                    serde_json::Value::String(p.status.tag().into()),
                );
                obj.insert(
                    "message".into(),
                    serde_json::Value::String(p.message.clone()),
                );
                if let Some(d) = &p.detail {
                    obj.insert("detail".into(), serde_json::Value::String(d.clone()));
                }
                serde_json::Value::Object(obj)
            })
            .collect();
        serde_json::json!({
            "ok": !self.has_errors(),
            "probes": probes,
        })
    }
}

// ── Cross-platform probes ─────────────────────────────────────────────────

/// Probe: version + target triple — the same string `brigadier-cu --version`
/// returns, plus the build-time target so the user can sanity-check arch.
fn probe_version() -> Probe {
    let version = env!("CARGO_PKG_VERSION");
    let target = build_target_triple();
    Probe::ok("binary", format!("brigadier-cu {version} ({target})"))
}

/// Build-time target triple. We don't have a `built` crate dependency, so
/// we synthesise `<arch>-<os>` which is enough to disambiguate the host
/// without adding a new transitive dep.
fn build_target_triple() -> String {
    format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS)
}

/// Probe: where the binary lives on disk. Resolves symlinks (e.g. the
/// `~/.local/bin/brigadier-cu -> packages/current/brigadier-cu` chain) so the
/// user sees the actual versioned release dir.
fn probe_install_layout() -> Probe {
    let exe = std::env::current_exe();
    match exe {
        Err(e) => Probe::err("install dir", format!("could not resolve current_exe: {e}")),
        Ok(path) => {
            let canonical = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
            let mut detail = String::new();
            if path != canonical {
                detail.push_str(&format!("argv exe:  {}\n", path.display()));
                detail.push_str(&format!("resolved:  {}", canonical.display()));
            } else {
                detail.push_str(&format!("path: {}", canonical.display()));
            }
            Probe::ok("install dir", canonical.display().to_string()).with_detail(detail)
        }
    }
}

/// Probe: package-home directory (`~/.brigadier-cu`) — counts cached
/// release dirs so the user can sanity-check
/// `BRIGADIER_CU_KEEP_VERSIONS` is doing the right thing. Renamed from
/// `.brigadier-cu/` in v0.2.16 to match the install-path rename (PR #1644).
fn probe_home_dir() -> Probe {
    let home = match home_dir() {
        Some(h) => h,
        None => return Probe::warn("home dir", "neither HOME nor USERPROFILE set"),
    };
    let cu_home = home.join(".brigadier-cu");
    if !cu_home.exists() {
        return Probe::warn(
            "home dir",
            format!(
                "{} does not exist yet (created on first run)",
                cu_home.display()
            ),
        );
    }
    let releases = cu_home.join("packages").join("releases");
    // Only count subdirectories — bare files inside `releases/` (stray
    // download artifacts, lock files, .DS_Store) are not actual cached
    // versions and shouldn't inflate the "release dir(s) cached" metric.
    let release_count = std::fs::read_dir(&releases)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| entry.metadata().map(|m| m.is_dir()).unwrap_or(false))
                .count()
        })
        .unwrap_or(0);
    Probe::ok(
        "home dir",
        format!(
            "{} ({release_count} release dir{} cached)",
            cu_home.display(),
            if release_count == 1 { "" } else { "s" },
        ),
    )
}

/// `$HOME` on Unix, `%USERPROFILE%` on Windows.
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

// ── Platform-specific probe entry-points ──────────────────────────────────

#[cfg(target_os = "macos")]
fn append_platform_probes(report: &mut Report) {
    report.push(Probe::ok(
        "TCC + cdhash report",
        "for a full bundle / signature / TCC dump, run `brigadier-cu diagnose`",
    ));
}

// ── Public entry point ────────────────────────────────────────────────────

/// Run every probe and return the aggregated report.
pub fn run() -> Report {
    let mut report = Report::default();
    report.push(probe_version());
    report.push(probe_install_layout());
    report.push(probe_home_dir());
    append_platform_probes(&mut report);
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_to_text_renders_status_tag_and_message() {
        let mut r = Report::default();
        r.push(Probe::ok("a", "alpha"));
        r.push(Probe::warn("b", "bravo").with_detail("two\nlines"));
        r.push(Probe::err("c", "charlie"));
        let text = r.to_text();
        assert!(text.contains("[ok  ] a: alpha"));
        assert!(text.contains("[warn] b: bravo"));
        assert!(text.contains("         two"));
        assert!(text.contains("         lines"));
        assert!(text.contains("[err ] c: charlie"));
    }

    #[test]
    fn report_to_json_marks_ok_false_when_any_error() {
        let mut r = Report::default();
        r.push(Probe::ok("a", "alpha"));
        r.push(Probe::err("b", "bravo"));
        let json = r.to_json();
        assert_eq!(json["ok"], serde_json::Value::Bool(false));
        assert_eq!(json["probes"].as_array().unwrap().len(), 2);
        assert_eq!(json["probes"][1]["status"], "err");
    }

    #[test]
    fn report_to_json_marks_ok_true_when_only_warnings() {
        let mut r = Report::default();
        r.push(Probe::ok("a", "alpha"));
        r.push(Probe::warn("b", "bravo"));
        let json = r.to_json();
        // Warnings do not fail the run — exit-code-driving flag stays true.
        assert_eq!(json["ok"], serde_json::Value::Bool(true));
    }

    #[test]
    fn cross_platform_probes_always_emit_something() {
        // Smoke test: run the cross-platform probes and confirm they all
        // produced a probe (no silent dropouts).
        let v = probe_version();
        assert_eq!(v.label, "binary");
        let i = probe_install_layout();
        assert_eq!(i.label, "install dir");
        let h = probe_home_dir();
        assert_eq!(h.label, "home dir");
    }

    #[test]
    fn run_emits_at_least_cross_platform_probes() {
        let report = run();
        // 3 cross-platform + at least 1 platform-specific.
        assert!(report.probes.len() >= 4, "got {}", report.probes.len());
    }
}
