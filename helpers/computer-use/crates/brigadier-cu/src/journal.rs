//! Opt-in local reliability journal.
//!
//! When enabled on a machine, every MCP `tools/call` that passes through the
//! stdio proxy appends one JSON line (tool, outcome, duration, error text) to
//! a local file, and `brigadier-cu complain` appends agent-written problem reports
//! to the same file. A maintainer tails it to find and fix driver failures.
//!
//! The journal is off by default and never leaves the machine. It is enabled
//! by `brigadier-cu journal enable` (a marker file) or `BRIGADIER_CU_JOURNAL=1`, and
//! `BRIGADIER_CU_JOURNAL=0` force-disables it. Argument values are never written,
//! only argument names, so typed text and URLs stay out of the file.

use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

const SCHEMA_VERSION: u64 = 1;
const MAX_JOURNAL_BYTES: u64 = 32 * 1024 * 1024;
const MAX_TEXT_CHARS: usize = 4000;

/// Marker file whose presence enables the journal on this machine.
pub fn marker_path() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("BRIGADIER_CU_JOURNAL_DIR") {
        return Some(PathBuf::from(dir).join("enabled"));
    }
    Some(crate::home::home().join("journal-enabled"))
}

/// The append-only JSONL journal.
pub fn journal_path() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("BRIGADIER_CU_JOURNAL_DIR") {
        return Some(PathBuf::from(dir).join("journal.jsonl"));
    }
    Some(crate::home::home().join("journal.jsonl"))
}

/// Whether this process should write journal entries.
pub fn enabled() -> bool {
    match std::env::var("BRIGADIER_CU_JOURNAL")
        .ok()
        .as_deref()
        .map(str::trim)
    {
        Some("1") | Some("true") | Some("yes") | Some("on") => return true,
        Some("0") | Some("false") | Some("no") | Some("off") => return false,
        _ => {}
    }
    marker_path().is_some_and(|p| p.exists())
}

fn now_rfc3339() -> String {
    let now = time::OffsetDateTime::now_utc();
    now.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| {
            let secs = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            secs.to_string()
        })
}

fn truncate(text: &str) -> String {
    if text.chars().count() <= MAX_TEXT_CHARS {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(MAX_TEXT_CHARS).collect();
    out.push_str("…[truncated]");
    out
}

/// Process name of the MCP client (claude, codex, ...) that spawned this
/// proxy. Resolved once per process.
fn client_name() -> &'static str {
    static NAME: OnceLock<String> = OnceLock::new();
    NAME.get_or_init(|| {
        #[cfg(target_os = "macos")]
        let name = i32::try_from(std::os::unix::process::parent_id())
            .ok()
            .and_then(crate::process_name::process_name);
        #[cfg(not(target_os = "macos"))]
        let name: Option<String> = None;
        name.map(|s| s.rsplit('/').next().unwrap_or(&s).to_owned())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "unknown".to_owned())
    })
}

/// Host app scope, e.g. `ai.brigadier.app` or a tagged debug bundle, derived
/// from the state dir Brigadier injects into every MCP launch.
fn host_scope() -> Option<String> {
    let dir = std::env::var("BRIGADIER_CU_STATE_DIR").ok()?;
    let parts: Vec<&str> = dir.split('/').collect();
    let runtime = parts.iter().position(|p| *p == "runtime")?;
    parts.get(runtime + 1).map(|s| (*s).to_owned())
}

fn base_record(kind: &str) -> serde_json::Map<String, Value> {
    let mut m = serde_json::Map::new();
    m.insert("v".into(), json!(SCHEMA_VERSION));
    m.insert("ts".into(), json!(now_rfc3339()));
    m.insert("kind".into(), json!(kind));
    m.insert("version".into(), json!(env!("CARGO_PKG_VERSION")));
    m.insert("pid".into(), json!(std::process::id()));
    m.insert("client".into(), json!(client_name()));
    if let Ok(session) = std::env::var("BRIGADIER_CU_DEFAULT_SESSION") {
        m.insert("default_session".into(), json!(session));
    }
    if let Some(scope) = host_scope() {
        m.insert("host".into(), json!(scope));
    }
    // Link a record back to the agent transcript and Brigadier surface.
    for (field, var) in [
        ("surface", "BRIGADIER_SURFACE_ID"),
        ("claude_session", "CLAUDE_CODE_SESSION_ID"),
        ("codex_thread", "CODEX_THREAD_ID"),
    ] {
        if let Ok(value) = std::env::var(var) {
            if !value.is_empty() {
                m.insert(field.into(), json!(value));
            }
        }
    }
    m
}

fn append(record: serde_json::Map<String, Value>) -> std::io::Result<PathBuf> {
    let path = journal_path()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no HOME"))?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if fs::metadata(&path).map(|m| m.len()).unwrap_or(0) > MAX_JOURNAL_BYTES {
        let _ = fs::rename(&path, path.with_extension("1.jsonl"));
    }
    let mut line = serde_json::to_string(&Value::Object(record))
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    line.push('\n');
    // One write(2) on an O_APPEND fd so concurrent proxies never interleave
    // partial lines.
    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
    file.write_all(line.as_bytes())?;
    Ok(path)
}

/// Outcome of one proxied tool call, extracted from the JSON-RPC response.
#[derive(Debug, PartialEq)]
pub struct CallOutcome {
    pub ok: bool,
    pub error: Option<String>,
    pub code: Option<String>,
}

/// Classify a serialized JSON-RPC response. A JSON-RPC `error` and an MCP
/// `result.isError` both count as failures; the error text is the first text
/// content block (or the RPC error message).
pub fn outcome_from_response(response: &Value) -> CallOutcome {
    if let Some(err) = response.get("error") {
        return CallOutcome {
            ok: false,
            error: err.get("message").and_then(Value::as_str).map(truncate),
            code: err.get("code").map(|c| c.to_string()),
        };
    }
    let result = response.get("result");
    let is_error = result
        .and_then(|r| r.get("isError"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let structured = result.and_then(|r| r.get("structuredContent"));
    let code = structured
        .and_then(|s| s.get("code").or_else(|| s.get("error")))
        .and_then(Value::as_str)
        .map(str::to_owned);
    if !is_error {
        return CallOutcome {
            ok: true,
            error: None,
            code: None,
        };
    }
    let text = result
        .and_then(|r| r.get("content"))
        .and_then(Value::as_array)
        .and_then(|blocks| {
            blocks
                .iter()
                .find_map(|b| b.get("text").and_then(Value::as_str))
        })
        .map(truncate);
    CallOutcome {
        ok: false,
        error: text,
        code,
    }
}

/// Record one proxied `tools/call`. Never fails the caller.
pub fn record_tool_call(
    tool: &str,
    args: Option<&Value>,
    session: &str,
    profile: &str,
    duration: Duration,
    response: &Value,
) {
    if !enabled() {
        return;
    }
    let outcome = outcome_from_response(response);
    let mut m = base_record("call");
    m.insert("tool".into(), json!(tool));
    m.insert("session".into(), json!(session));
    m.insert("profile".into(), json!(profile));
    m.insert("duration_ms".into(), json!(duration.as_millis() as u64));
    m.insert("ok".into(), json!(outcome.ok));
    if let Some(error) = outcome.error {
        m.insert("error".into(), json!(error));
    }
    if let Some(code) = outcome.code {
        m.insert("code".into(), json!(code));
    }
    if let Some(keys) = args.and_then(Value::as_object) {
        let names: Vec<&String> = keys.keys().collect();
        m.insert("arg_keys".into(), json!(names));
    }
    if let Err(e) = append(m) {
        tracing::debug!(error = %e, "journal append failed");
    }
}

/// Guidance appended to the MCP `instructions` only on machines that opted
/// in, so agents know where to report brigadier-cu problems.
pub const COMPLAINT_INSTRUCTIONS: &str = "Reliability journal is ON for this machine. \
When a brigadier-cu tool fails, hangs, returns a wrong or unverifiable result, or you \
fall back to another tool (screencapture, osascript, CGEvent), report it once from a \
shell: `brigadier-cu complain \"<tool>: what you did, what you expected, what happened\" \
--tool <tool>`. Keep it to one or two sentences; a maintainer reads these to fix the driver.";

/// Append [`COMPLAINT_INSTRUCTIONS`] to an initialize result when enabled.
pub fn with_complaint_instructions(mut result: Value) -> Value {
    if !enabled() {
        return result;
    }
    if let Some(obj) = result.as_object_mut() {
        let text = match obj.get("instructions").and_then(Value::as_str) {
            Some(existing) => format!("{existing}\n\n{COMPLAINT_INSTRUCTIONS}"),
            None => COMPLAINT_INSTRUCTIONS.to_owned(),
        };
        obj.insert("instructions".into(), json!(text));
    }
    result
}

/// Record a lifecycle problem that is not tied to one tool call.
pub fn record_event(event: &str, detail: &str) {
    if !enabled() {
        return;
    }
    let mut m = base_record("event");
    m.insert("event".into(), json!(event));
    m.insert("detail".into(), json!(truncate(detail)));
    let _ = append(m);
}

// ── CLI ─────────────────────────────────────────────────────────────────────

fn usage() -> ! {
    eprintln!(
        "Usage:\n  \
         brigadier-cu complain <text> [--tool NAME] [--session ID]\n  \
         brigadier-cu journal status|enable|disable|path\n  \
         brigadier-cu journal tail [-n N] [--errors] [--follow] [--json]\n  \
         brigadier-cu journal summary [--hours H] [--json]"
    );
    std::process::exit(64);
}

/// `brigadier-cu complain <text>`: an agent reports a problem it hit.
pub fn run_complain(text: String, tool: Option<String>, session: Option<String>) {
    let text = text.trim().to_owned();
    if text.is_empty() {
        usage();
    }
    if !enabled() {
        eprintln!(
            "brigadier-cu: the reliability journal is disabled on this machine; \
             the complaint was not recorded."
        );
        return;
    }
    let mut m = base_record("complaint");
    m.insert("text".into(), json!(truncate(&text)));
    if let Some(tool) = tool {
        m.insert("tool".into(), json!(tool));
    }
    if let Some(session) = session {
        m.insert("session".into(), json!(session));
    }
    if let Ok(cwd) = std::env::current_dir() {
        m.insert("cwd".into(), json!(cwd.display().to_string()));
    }
    match append(m) {
        Ok(path) => println!("recorded in {}", path.display()),
        Err(e) => {
            eprintln!("brigadier-cu: could not record the complaint: {e}");
            std::process::exit(74);
        }
    }
}

fn read_records(path: &PathBuf) -> Vec<Value> {
    let Ok(file) = fs::File::open(path) else {
        return Vec::new();
    };
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|l| serde_json::from_str(&l).ok())
        .collect()
}

fn is_problem(record: &Value) -> bool {
    match record.get("kind").and_then(Value::as_str) {
        Some("call") => record.get("ok").and_then(Value::as_bool) == Some(false),
        Some(_) => true,
        None => false,
    }
}

fn format_record(record: &Value) -> String {
    let s = |k: &str| record.get(k).and_then(Value::as_str).unwrap_or("");
    match s("kind") {
        "call" => {
            let ms = record
                .get("duration_ms")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let ok = record.get("ok").and_then(Value::as_bool) == Some(true);
            format!(
                "{} {:<6} {:<5} {:>6}ms {} {}",
                s("ts"),
                s("client"),
                if ok { "ok" } else { "ERROR" },
                ms,
                s("tool"),
                s("error").replace('\n', " ")
            )
        }
        "complaint" => format!(
            "{} {:<6} COMPLAINT {} {}",
            s("ts"),
            s("client"),
            s("tool"),
            s("text").replace('\n', " ")
        ),
        _ => format!(
            "{} {:<6} EVENT {} {}",
            s("ts"),
            s("client"),
            s("event"),
            s("detail")
        ),
    }
}

fn print_record(record: &Value, json: bool) {
    if json {
        println!("{record}");
    } else {
        println!("{}", format_record(record));
    }
}

/// Strip volatile tokens so equal failures group together.
fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_digit = false;
    for c in text.chars() {
        if c.is_ascii_digit() {
            if !last_digit {
                out.push('#');
            }
            last_digit = true;
        } else {
            last_digit = false;
            out.push(c);
        }
    }
    out.chars().take(160).collect()
}

fn run_summary(path: &PathBuf, hours: u64, json: bool) {
    let cutoff = time::OffsetDateTime::now_utc() - time::Duration::hours(hours as i64);
    let records: Vec<Value> = read_records(path)
        .into_iter()
        .filter(|r| {
            r.get("ts")
                .and_then(Value::as_str)
                .and_then(|t| {
                    time::OffsetDateTime::parse(t, &time::format_description::well_known::Rfc3339)
                        .ok()
                })
                .is_some_and(|t| t >= cutoff)
        })
        .collect();
    let mut per_tool: std::collections::BTreeMap<String, (u64, u64)> = Default::default();
    let mut clusters: std::collections::BTreeMap<String, u64> = Default::default();
    let mut complaints = 0u64;
    for r in &records {
        match r.get("kind").and_then(Value::as_str) {
            Some("call") => {
                let tool = r
                    .get("tool")
                    .and_then(Value::as_str)
                    .unwrap_or("?")
                    .to_owned();
                let entry = per_tool.entry(tool.clone()).or_default();
                entry.0 += 1;
                if r.get("ok").and_then(Value::as_bool) == Some(false) {
                    entry.1 += 1;
                    let err = r
                        .get("error")
                        .and_then(Value::as_str)
                        .unwrap_or("(no text)");
                    *clusters
                        .entry(format!("{tool}: {}", normalize(err)))
                        .or_default() += 1;
                }
            }
            Some("complaint") => complaints += 1,
            _ => {}
        }
    }
    let mut ranked: Vec<(String, u64)> = clusters.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1));
    if json {
        let tools: serde_json::Map<String, Value> = per_tool
            .iter()
            .map(|(k, (calls, errors))| (k.clone(), json!({"calls": calls, "errors": errors})))
            .collect();
        let top: Vec<Value> = ranked
            .iter()
            .take(30)
            .map(|(k, n)| json!({"cluster": k, "count": n}))
            .collect();
        println!(
            "{}",
            json!({"hours": hours, "records": records.len(), "complaints": complaints,
                   "tools": tools, "top_errors": top})
        );
        return;
    }
    let calls: u64 = per_tool.values().map(|v| v.0).sum();
    let errors: u64 = per_tool.values().map(|v| v.1).sum();
    println!("last {hours}h: {calls} calls, {errors} errors, {complaints} complaints");
    for (tool, (c, e)) in &per_tool {
        if *e > 0 {
            println!("  {tool:<28} {e:>5}/{c:<5} failed");
        }
    }
    if !ranked.is_empty() {
        println!("top errors:");
        for (k, n) in ranked.iter().take(20) {
            println!("  {n:>5}  {k}");
        }
    }
}

fn run_tail(path: &PathBuf, count: usize, errors_only: bool, follow: bool, json: bool) {
    let records: Vec<Value> = read_records(path)
        .into_iter()
        .filter(|r| !errors_only || is_problem(r))
        .collect();
    let skip = records.len().saturating_sub(count);
    for r in &records[skip..] {
        print_record(r, json);
    }
    if !follow {
        return;
    }
    let mut offset = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let mut partial = String::new();
    loop {
        std::thread::sleep(Duration::from_millis(500));
        let len = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        if len < offset {
            offset = 0; // rotated
        }
        if len == offset {
            continue;
        }
        let Ok(mut file) = fs::File::open(path) else {
            continue;
        };
        if file.seek(SeekFrom::Start(offset)).is_err() {
            continue;
        }
        let mut buf = String::new();
        if std::io::Read::read_to_string(&mut file, &mut buf).is_err() {
            continue;
        }
        offset += buf.len() as u64;
        partial.push_str(&buf);
        while let Some(pos) = partial.find('\n') {
            let line: String = partial.drain(..=pos).collect();
            if let Ok(r) = serde_json::from_str::<Value>(line.trim()) {
                if !errors_only || is_problem(&r) {
                    print_record(&r, json);
                    let _ = std::io::stdout().flush();
                }
            }
        }
    }
}

/// `brigadier-cu journal <subcommand>`.
pub fn run_journal(subcommand: &str, args: &[String]) {
    let flag = |name: &str| args.iter().any(|a| a == name);
    let value = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .and_then(|v| v.parse::<u64>().ok())
    };
    let Some(path) = journal_path() else {
        eprintln!("brigadier-cu: HOME is not set");
        std::process::exit(78);
    };
    match subcommand {
        "status" => {
            println!(
                "journal: {}\nfile: {}\nmarker: {}",
                if enabled() { "enabled" } else { "disabled" },
                path.display(),
                marker_path()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default()
            );
        }
        "path" => println!("{}", path.display()),
        "enable" => {
            let Some(marker) = marker_path() else {
                std::process::exit(78)
            };
            if let Some(parent) = marker.parent() {
                let _ = fs::create_dir_all(parent);
            }
            if let Err(e) = fs::write(&marker, b"") {
                eprintln!("brigadier-cu: could not enable the journal: {e}");
                std::process::exit(74);
            }
            println!("journal enabled; writing to {}", path.display());
        }
        "disable" => {
            if let Some(marker) = marker_path() {
                let _ = fs::remove_file(marker);
            }
            println!("journal disabled");
        }
        "tail" => run_tail(
            &path,
            value("-n").unwrap_or(40) as usize,
            flag("--errors"),
            flag("--follow") || flag("-f"),
            flag("--json"),
        ),
        "summary" => run_summary(&path, value("--hours").unwrap_or(24), flag("--json")),
        _ => usage(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rpc_error_is_a_failure_with_message() {
        let r = json!({"jsonrpc":"2.0","id":1,"error":{"code":-32603,"message":"daemon failed"}});
        let o = outcome_from_response(&r);
        assert!(!o.ok);
        assert_eq!(o.error.as_deref(), Some("daemon failed"));
        assert_eq!(o.code.as_deref(), Some("-32603"));
    }

    #[test]
    fn tool_is_error_uses_first_text_block() {
        let r = json!({"id":1,"result":{"isError":true,
            "content":[{"type":"image"},{"type":"text","text":"stale element"}],
            "structuredContent":{"code":"stale_token"}}});
        let o = outcome_from_response(&r);
        assert!(!o.ok);
        assert_eq!(o.error.as_deref(), Some("stale element"));
        assert_eq!(o.code.as_deref(), Some("stale_token"));
    }

    #[test]
    fn success_is_ok() {
        let r = json!({"id":1,"result":{"content":[{"type":"text","text":"done"}]}});
        assert_eq!(
            outcome_from_response(&r),
            CallOutcome {
                ok: true,
                error: None,
                code: None
            }
        );
    }

    #[test]
    fn normalize_collapses_numbers() {
        assert_eq!(normalize("pid 1234 window 55"), "pid # window #");
    }

    #[test]
    fn records_calls_and_complaints_only_when_enabled() {
        let dir = std::env::temp_dir().join(format!("cu-journal-{}", uuid::Uuid::new_v4()));
        std::env::set_var("BRIGADIER_CU_JOURNAL_DIR", &dir);
        std::env::remove_var("BRIGADIER_CU_JOURNAL");
        let resp = json!({"id":1,"error":{"code":-1,"message":"boom"}});
        record_tool_call(
            "click",
            None,
            "s",
            "native",
            Duration::from_millis(5),
            &resp,
        );
        assert!(!dir.join("journal.jsonl").exists());

        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("enabled"), b"").unwrap();
        record_tool_call(
            "click",
            Some(&json!({"pid": 1, "text": "secret"})),
            "s",
            "native",
            Duration::from_millis(5),
            &resp,
        );
        let records = read_records(&dir.join("journal.jsonl"));
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["tool"], "click");
        assert_eq!(records[0]["ok"], false);
        assert_eq!(records[0]["error"], "boom");
        let line = fs::read_to_string(dir.join("journal.jsonl")).unwrap();
        assert!(
            !line.contains("secret"),
            "argument values must never be journaled"
        );
        std::env::remove_var("BRIGADIER_CU_JOURNAL_DIR");
        let _ = fs::remove_dir_all(dir);
    }
}
