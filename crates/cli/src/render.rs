//! The text output of each command (`--json` prints the reply's output as is).

use base64::Engine as _;

use crate::protocol::{
    Answer, AskOutput, Changes, CloseOutput, Item, ItemKind, LibraryOutput, QueueOutput,
    QueueState, ReadOutput, SendOutput, SessionsOutput, ShowOutput, StartOutput, ToolOutput,
    WaitOutput, WhyBy, WhyOutput, WorkerRow, WorkersOutput,
};

/// Lays out rows in columns, two spaces apart; the last column is never padded.
fn table(rows: &[Vec<String>]) -> String {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..columns)
        .map(|column| {
            rows.iter()
                .filter_map(|row| row.get(column))
                .map(|cell| cell.chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    let mut out = String::new();
    for row in rows {
        let mut line = String::new();
        for (column, cell) in row.iter().enumerate() {
            if column + 1 == row.len() {
                line.push_str(cell);
            } else {
                line.push_str(cell);
                let pad = widths[column] - cell.chars().count() + 2;
                line.extend(std::iter::repeat_n(' ', pad));
            }
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

fn changes(changes: Option<Changes>) -> String {
    match changes {
        Some(changes) => format!("+{} −{}", changes.insertions, changes.deletions),
        None => "-".into(),
    }
}

pub fn sessions(output: &SessionsOutput) -> String {
    let mut out = String::new();
    if let Some(project) = &output.project {
        out.push_str(&format!("{project}\n"));
    }
    if output.sessions.is_empty() {
        out.push_str("No sessions.\n");
        return out;
    }
    let all = output
        .sessions
        .iter()
        .any(|session| session.project.is_some());
    let mut head: Vec<String> = vec![
        "ID".into(),
        "STATE".into(),
        "CHANGES".into(),
        "WORKERS".into(),
        "BRANCH".into(),
    ];
    if all {
        head.push("PROJECT".into());
    }
    head.push("TITLE".into());
    let mut rows = vec![head];
    for session in &output.sessions {
        let title = if session.caller {
            format!("{} (this session)", session.title)
        } else {
            session.title.clone()
        };
        let mut row = vec![
            session.id.clone(),
            session.state.word().into(),
            changes(session.changes),
            session.workers.to_string(),
            session.branch.clone().unwrap_or_else(|| "-".into()),
        ];
        if all {
            row.push(session.project.clone().unwrap_or_else(|| "-".into()));
        }
        row.push(title);
        rows.push(row);
    }
    out.push_str(&table(&rows));
    out
}

/// How full a worker's context is: "120k of 1M", "120k" without a known window, "-" unknown.
fn context(context: Option<crate::protocol::ContextSize>) -> String {
    fn tokens(n: i64) -> String {
        if n >= 1_000_000 && n % 1_000_000 == 0 {
            format!("{}M", n / 1_000_000)
        } else if n >= 1_000 {
            format!("{}k", n / 1_000)
        } else {
            n.to_string()
        }
    }
    match context {
        None => "-".into(),
        Some(c) => match c.window_tokens {
            Some(window) => format!("{} of {}", tokens(c.used_tokens), tokens(window)),
            None => tokens(c.used_tokens),
        },
    }
}

fn worker_rows(workers: &[WorkerRow]) -> String {
    let mut rows = vec![vec![
        "ID".into(),
        "#".into(),
        "STATE".into(),
        "AGENT".into(),
        "MODEL".into(),
        "CHANGES".into(),
        "CONTEXT".into(),
        "TITLE".into(),
    ]];
    for worker in workers {
        let model = match (&worker.model, &worker.effort) {
            (Some(model), Some(effort)) => format!("{model} ({effort})"),
            (Some(model), None) => model.clone(),
            (None, Some(effort)) => format!("default ({effort})"),
            (None, None) => "default".into(),
        };
        rows.push(vec![
            worker.id.clone(),
            worker.number.to_string(),
            worker.state.word().into(),
            worker.agent.word().into(),
            model,
            changes(worker.changes),
            context(worker.context),
            worker.title.clone(),
        ]);
    }
    table(&rows)
}

pub fn workers(output: &WorkersOutput) -> String {
    if output.workers.is_empty() {
        return "This session has no workers.\n".into();
    }
    worker_rows(&output.workers)
}

pub fn show(output: &ShowOutput) -> String {
    let mut rows: Vec<Vec<String>> = vec![
        vec!["id".into(), output.id.clone()],
        vec!["kind".into(), output.kind.word().into()],
        vec!["title".into(), output.title.clone()],
        vec!["state".into(), output.state.clone()],
    ];
    if let Some(parent) = &output.parent {
        rows.push(vec!["session".into(), parent.clone()]);
    }
    if let Some(number) = output.number {
        rows.push(vec!["number".into(), number.to_string()]);
    }
    rows.push(vec!["agent".into(), output.agent.word().into()]);
    rows.push(vec![
        "model".into(),
        output.model.clone().unwrap_or_else(|| "default".into()),
    ]);
    if let Some(effort) = &output.effort {
        rows.push(vec!["effort".into(), effort.clone()]);
    }
    if let Some(checkout) = output.checkout {
        let checkout = match checkout {
            crate::protocol::Checkout::Worktree => "own worktree",
            crate::protocol::Checkout::Local => "local checkout",
        };
        rows.push(vec!["checkout".into(), checkout.into()]);
    }
    if let Some(branch) = &output.branch {
        rows.push(vec!["branch".into(), branch.clone()]);
    }
    if let Some(base) = &output.base {
        rows.push(vec!["base".into(), base.clone()]);
    }
    if let Some(changed) = output.changes {
        rows.push(vec![
            "changes".into(),
            format!("{} in {} files", changes(Some(changed)), changed.files),
        ]);
    }
    if let Some(waiting) = output.waiting
        && waiting.questions + waiting.approvals > 0
    {
        rows.push(vec![
            "waiting".into(),
            format!(
                "{} questions, {} approvals",
                waiting.questions, waiting.approvals
            ),
        ]);
    }
    if let Some(error) = &output.error {
        rows.push(vec!["error".into(), error.clone()]);
    }
    let mut out = table(&rows);
    if let Some(task) = &output.task {
        out.push_str(&format!("\nTask:\n{}\n", task.trim_end()));
    }
    if let Some(report) = &output.report {
        out.push_str(&format!("\nReport:\n{}\n", report.trim_end()));
    }
    if let Some(workers) = &output.workers
        && !workers.is_empty()
    {
        out.push_str("\nWorkers:\n");
        out.push_str(&worker_rows(workers));
    }
    out
}

fn item(item: &Item) -> String {
    let mut out = match item.kind {
        ItemKind::Message => {
            let who = match item.role {
                Some(crate::protocol::Role::User) => "user",
                Some(crate::protocol::Role::Assistant) | None => "assistant",
                Some(crate::protocol::Role::System) => "system",
                Some(crate::protocol::Role::Peer) => "other session",
            };
            format!("[{who}]\n{}\n", item.text.trim_end())
        }
        ItemKind::Reasoning => format!("[thinking] {}\n", item.text.trim_end()),
        ItemKind::Command => format!("$ {}\n", item.text.trim_end()),
        ItemKind::Tool => format!("[tool] {}\n", item.text.trim_end()),
        ItemKind::TurnEnd => format!("[turn ended] {}\n", item.text.trim_end()),
        ItemKind::Error => format!("[error] {}\n", item.text.trim_end()),
    };
    if let Some(output) = &item.output
        && !output.trim().is_empty()
    {
        for line in output.trim_end().lines() {
            out.push_str(&format!("  {line}\n"));
        }
    }
    match (item.status, item.exit) {
        (_, Some(exit)) => out.push_str(&format!("  (exit {exit})\n")),
        (Some(crate::protocol::ItemStatus::Running), None) => out.push_str("  (running)\n"),
        (Some(crate::protocol::ItemStatus::Failed), None) => out.push_str("  (failed)\n"),
        _ => {}
    }
    out
}

pub fn read(output: &ReadOutput) -> String {
    let mut out = format!("{} {} · {}\n", output.kind.word(), output.id, output.state);
    if output.items.is_empty() {
        out.push_str("Nothing yet.\n");
        return out;
    }
    if output.has_more {
        out.push_str("(earlier items not shown)\n");
    }
    for entry in &output.items {
        out.push('\n');
        out.push_str(&item(entry));
    }
    out
}

pub fn start(output: &StartOutput) -> String {
    let worker = &output.worker;
    let place = match (&worker.branch, &output.base) {
        (Some(branch), Some(base)) => format!("on {branch} from {base}"),
        (Some(branch), None) => format!("on {branch}"),
        (None, Some(base)) => format!("in a new worktree from {base}"),
        (None, None) => "in this session's checkout".into(),
    };
    let row = output.row.as_ref().map_or(String::new(), |row| {
        let whose = match row.source.as_str() {
            "user" => "Your row",
            _ => "The shipped row",
        };
        let picked = match row.picked.as_slice() {
            [] => "nothing; your flags set it all".to_owned(),
            [one] => format!("its {one}"),
            [rest @ .., last] => format!("its {} and {last}", rest.join(", ")),
        };
        format!("{whose} \"{}\" picked {picked}.\n", row.work)
    });
    format!(
        "{}\nStarted worker {} ({}, {}{}), {place}, permission {}.\n{row}Keep `brigadier wait` \
         running in the background: it returns with its events and wakes you.\n",
        worker.id,
        worker.number,
        worker.agent.word(),
        worker.model.as_deref().unwrap_or("default model"),
        worker
            .effort
            .as_deref()
            .map_or(String::new(), |effort| format!(", {effort} effort")),
        output.permission.word(),
    )
}

pub fn send(output: &SendOutput) -> String {
    match output.answered {
        Some(Answer::Allow) => format!("Allowed what {} asked.\n", output.id),
        Some(Answer::Deny) => format!("Denied what {} asked.\n", output.id),
        None => {
            let sent = match &output.message {
                Some(message) => format!("Sent {message} to session {}", output.id),
                None => format!("Sent to {}", output.id),
            };
            if output.queued {
                format!("{sent}: queued into its turn.\n")
            } else {
                format!("{sent}: it works on it.\n")
            }
        }
    }
}

pub fn wait(output: &WaitOutput) -> String {
    if output.events.is_empty() {
        return if output.timed_out {
            "Nothing yet (timed out).\n".into()
        } else {
            "Nothing to wait for.\n".into()
        };
    }
    let mut out = String::new();
    for (at, event) in output.events.iter().enumerate() {
        if at > 0 {
            out.push('\n');
        }
        let mut head = format!("{} {}", event.id, event.event.word());
        if !event.event_id.is_empty() {
            head.push_str(&format!(" ({})", event.event_id));
        }
        out.push_str(&head);
        out.push('\n');
        if let Some(asks) = &event.asks {
            out.push_str(&format!(
                "asks: {}\n`brigadier send {} --allow` or `--deny` answers.\n",
                asks.trim_end(),
                event.id
            ));
        }
        if let Some(message) = &event.message {
            out.push_str(message.trim_end());
            out.push('\n');
        }
    }
    out
}

pub fn continue_on(output: &crate::protocol::ContinueOnOutput) -> String {
    use crate::protocol::ContinueWhen;
    let agent = match output.agent {
        crate::protocol::Agent::Claude => "Claude Code",
        crate::protocol::Agent::Codex => "Codex",
    };
    let on = match (&output.model, &output.effort) {
        (Some(model), Some(effort)) => format!(" ({model}, {effort})"),
        (Some(model), None) => format!(" ({model})"),
        (None, Some(effort)) => format!(" (its default model, {effort})"),
        (None, None) => " (its default model)".to_owned(),
    };
    match output.when {
        ContinueWhen::Already => format!("This session runs on {agent} already.\n"),
        ContinueWhen::Now => {
            format!("Continuing on {agent}{on}: the hand-off note first, then the new session.\n")
        }
        ContinueWhen::AfterTurn => format!(
            "Continues on {agent}{on} once this turn ends: the hand-off note first, then the \
             new session.\n"
        ),
    }
}

pub fn queue(output: &QueueOutput) -> String {
    let id = &output.id;
    match output.state {
        QueueState::Joined if output.already => format!(
            "{id} had already joined this work, unjudged: it is the user's message here; act on \
             it and answer it with the rest.\n"
        ),
        QueueState::Joined => {
            format!("{id} joined this work: act on it and answer it with the rest.\n")
        }
        QueueState::Waiting => {
            format!(
                "{id} waits in the queue and comes again on its own turn once this work is done.\n"
            )
        }
        QueueState::SentToWorker => format!(
            "{id} went to worker {}; the user sees where it went.\n",
            output.worker.as_deref().unwrap_or("?")
        ),
        QueueState::Offered => format!(
            "{id} waits in the queue offering the user \"Open as a new session\" or \"Run \
             here\" until they pick.\n"
        ),
    }
}

pub fn close(output: &CloseOutput) -> String {
    if output.closed {
        format!("Closed {}.\n", output.id)
    } else {
        format!("{} had already ended.\n", output.id)
    }
}

/// `preview`, `browser`, `app` and `page`: the tool's answer, and where its screenshots went.
/// A project folder by its last part.
fn folder_name(path: &str) -> &str {
    path.rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or(path)
}

pub fn library(output: &LibraryOutput) -> String {
    let mut out = String::new();
    if output.action == "list" && output.items.is_empty() {
        out.push_str(
            "The Library is empty: `brigadier library add` puts a skill or MCP server in it.\n",
        );
    }
    let mut rows = Vec::new();
    for item in &output.items {
        let names = |paths: &[String]| {
            paths
                .iter()
                .map(|path| folder_name(path))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let place = match (
            item.everywhere,
            item.on_in.is_empty(),
            item.off_in.is_empty(),
        ) {
            (true, _, true) => "every project".to_owned(),
            (true, _, false) => format!("every project but {}", names(&item.off_in)),
            (false, false, _) => format!("only in {}", names(&item.on_in)),
            (false, true, _) => "off".to_owned(),
        };
        let agents = item
            .agents
            .iter()
            .map(|agent| format!("{} {}", agent.agent, agent.state))
            .collect::<Vec<_>>()
            .join(", ");
        rows.push(vec![item.name.clone(), item.kind.clone(), place, agents]);
    }
    out.push_str(&table(&rows));
    for item in &output.items {
        if let Some(attention) = &item.attention {
            out.push_str(&format!("{}: {attention}\n", item.name));
        }
    }
    for agent in &output.agents {
        match &agent.message {
            Some(message) => {
                out.push_str(&format!("{}: {} — {message}\n", agent.agent, agent.outcome))
            }
            None => out.push_str(&format!("{}: {}\n", agent.agent, agent.outcome)),
        }
    }
    if let Some(warning) = &output.warning {
        out.push_str(&format!("{warning}\n"));
    }
    out
}

pub fn tool(output: &ToolOutput) -> String {
    let mut out = output.text.trim_end().to_owned();
    out.push('\n');
    for path in &output.saved {
        out.push_str(&format!("Screenshot saved to {path}\n"));
    }
    out
}

/// Writes the output's screenshots to files, for text output: to `out` (numbered after the
/// first), else to the session's temporary folder.
pub fn saved_images(
    mut output: ToolOutput,
    out: Option<&std::path::Path>,
) -> Result<ToolOutput, String> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    for (n, image) in std::mem::take(&mut output.images).iter().enumerate() {
        let Some((head, data)) = image.split_once(";base64,") else {
            return Err("a screenshot that isn't a data: URL".into());
        };
        let extension = match head.trim_start_matches("data:") {
            "image/jpeg" => "jpg",
            "image/webp" => "webp",
            _ => "png",
        };
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|err| err.to_string())?;
        let path = match out {
            Some(path) if n == 0 => path.to_owned(),
            Some(path) => {
                let stem = path.file_stem().unwrap_or_default().to_string_lossy();
                path.with_file_name(format!("{stem}-{}.{extension}", n + 1))
            }
            None => {
                std::env::temp_dir().join(format!("brigadier-screenshot-{stamp}-{n}.{extension}"))
            }
        };
        std::fs::write(&path, bytes)
            .map_err(|err| format!("could not save the screenshot to {}: {err}", path.display()))?;
        output.saved.push(path.display().to_string());
    }
    Ok(output)
}

/// `ask`: the user's answer, as they gave it.
pub fn ask(output: &AskOutput) -> String {
    format!("{}\n", output.answer.trim_end())
}

/// A usage window id as people say it ("five_hour" -> "5-hour").
/// `brigadier why`: the line, who wrote it, and what was asked and answered then.
pub fn why(output: &WhyOutput) -> String {
    let mut out = format!("{}:{}  {}\n", output.path, output.line, output.text.trim());
    let session = output.session.as_ref().map(|session| {
        format!(
            "session {} \"{}\" ({})",
            session.id,
            session.title,
            session.agent.word()
        )
    });
    let turn_line = |turn: &crate::protocol::WhyTurn| {
        let mut line = format!("turn {}", turn.number);
        if let Some(started) = turn.started_at_ms {
            line.push_str(&format!(", {}", local_time(started)));
        }
        line
    };
    match output.by {
        WhyBy::Turn => {
            if let (Some(turn), Some(session)) = (&output.turn, &session) {
                out.push_str(&format!("Written in {} of {session}.\n", turn_line(turn)));
            }
        }
        WhyBy::Worker => {
            if let Some(worker) = &output.worker {
                out.push_str(&format!(
                    "Written by worker {} \"{}\" ({})",
                    worker.id,
                    worker.title,
                    worker.agent.word()
                ));
                if let Some(session) = &session {
                    out.push_str(&format!(" of {session}"));
                }
                match &output.turn {
                    Some(turn) => out.push_str(&format!(", landed in {}.\n", turn_line(turn))),
                    None => out.push_str(", landed outside any turn.\n"),
                }
            }
        }
        WhyBy::Commit => out.push_str("No Brigadier turn wrote it.\n"),
        WhyBy::Uncommitted => out.push_str(
            "No Brigadier turn wrote it, and it isn't committed: it was edited outside any \
             session's turn.\n",
        ),
    }
    for moved in &output.moved_by {
        out.push_str(&format!(
            "Moved later by turn {} of session {}.\n",
            moved.turn, moved.session
        ));
    }
    if let Some(commit) = &output.commit {
        let id = &commit.id[..commit.id.len().min(12)];
        out.push_str(&format!(
            "Commit {id} by {}, {}: {}\n",
            commit.author,
            local_time(commit.at_ms),
            commit.summary
        ));
        if let Some(branch) = &commit.branch {
            out.push_str(&format!("From branch {branch}"));
            if let Some(merge) = &commit.merge {
                out.push_str(&format!(", merged in {}", &merge[..merge.len().min(12)]));
            }
            out.push_str(".\n");
        }
    }
    let block = |out: &mut String, head: &str, text: &str| {
        out.push_str(&format!("\n{head}\n"));
        for line in text.trim_end().lines() {
            out.push_str(&format!("  {line}\n"));
        }
    };
    if let Some(worker) = &output.worker {
        if let Some(gone) = &worker.gone {
            out.push_str(&format!("\n{gone}\n"));
        }
        if let Some(task) = &worker.task {
            block(&mut out, "Its task:", task);
        }
        if let Some(reply) = &worker.reply {
            block(
                &mut out,
                &reply_head("Its last reply", worker.reply_cut, &worker.read),
                reply,
            );
        }
        return out;
    }
    let Some(turn) = &output.turn else {
        return out;
    };
    if let Some(gone) = &turn.gone {
        out.push_str(&format!("\n{gone}\n"));
    }
    if let Some(message) = &turn.message {
        block(&mut out, "Message:", message);
    }
    if let Some(reply) = &turn.reply {
        block(
            &mut out,
            &reply_head("Reply", turn.reply_cut, &turn.read),
            reply,
        );
    }
    if !turn.ledger.is_empty() {
        out.push_str("\nLedger, written in that turn:\n");
        for entry in &turn.ledger {
            for line in entry.text.trim_end().lines() {
                out.push_str(&format!("  {}\n", line.trim_start()));
            }
        }
    }
    out
}

/// "Reply (its start; whole: brigadier brain read …):".
fn reply_head(what: &str, cut: bool, read: &Option<String>) -> String {
    match (cut, read) {
        (true, Some(read)) => format!("{what} (its start; whole: {read}):"),
        (true, None) => format!("{what} (its start):"),
        (false, Some(read)) => format!("{what} ({read}):"),
        (false, None) => format!("{what}:"),
    }
}

fn window_word(id: &str) -> std::borrow::Cow<'_, str> {
    match id {
        "five_hour" => "5-hour".into(),
        "seven_day" => "weekly".into(),
        "seven_day_fable" => "weekly (Fable)".into(),
        "primary" => "primary".into(),
        "secondary" => "secondary".into(),
        other => other.into(),
    }
}

/// How long ago a figure was read (" · read 40s ago"), or nothing when unknown.
fn read_ago(age_s: Option<i64>) -> String {
    age_s.map_or_else(String::new, |age| format!(" · read {age}s ago"))
}

/// A place in an order as people say it ("1st", "2nd", "3rd", "4th").
fn ordinal(n: u32) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

/// An epoch-ms instant as a local date and time ("2026-10-23 09:40"), or `unknown`.
fn local_time(at_ms: i64) -> String {
    jiff::Timestamp::from_millisecond(at_ms).map_or_else(
        |_| "unknown".into(),
        |at| {
            at.to_zoned(jiff::tz::TimeZone::system())
                .strftime("%Y-%m-%d %H:%M")
                .to_string()
        },
    )
}

/// A compact advice view. `--json` carries every row when the terminal summary is abbreviated.
pub fn models(output: &crate::models::ModelsOutput) -> String {
    let mut lines = vec![
        "Models and usage (advice; only your rules restrict starts)".to_owned(),
        format!(
            "Workers at once: up to {} (the user's setting, Settings › Configuration)",
            output.workers_at_once
        ),
    ];
    for agent in [
        crate::protocol::Agent::Claude,
        crate::protocol::Agent::Codex,
    ] {
        let models: Vec<_> = output
            .models
            .iter()
            .filter(|m| m.agent == agent)
            .map(|m| {
                let efforts = if m.efforts.is_empty() {
                    String::new()
                } else {
                    format!(" (efforts {})", m.efforts.join("/"))
                };
                format!("{}{efforts}{}", m.model, read_ago(m.age_s))
            })
            .collect();
        lines.push(format!(
            "{}: {}",
            agent.word(),
            if models.is_empty() {
                "not reported".into()
            } else {
                models.join(", ")
            }
        ));
    }
    if !output.agents.is_empty() {
        let switches: Vec<_> = output
            .agents
            .iter()
            .map(|a| {
                format!(
                    "{} {}",
                    a.agent.word(),
                    if a.switch_accounts {
                        "automatic"
                    } else {
                        "picked with Use"
                    }
                )
            })
            .collect();
        lines.push(format!(
            "Account switching: {}; another agent when all are out: {}",
            switches.join(", "),
            if output.switch_providers { "on" } else { "off" }
        ));
    }
    for account in &output.accounts {
        let usage: Vec<_> = account
            .usage
            .iter()
            .map(|w| {
                format!(
                    "{} {}% used, resets {}{}{}",
                    window_word(&w.window),
                    w.used_percent,
                    w.resets_at_ms.map_or_else(|| "unknown".into(), local_time),
                    if w.pooled {
                        " shared with other models"
                    } else {
                        ""
                    },
                    w.runway_ms.map_or_else(String::new, |ms| format!(
                        " runway {}m{}",
                        ms / 60_000,
                        if w.estimate_rough {
                            " (estimate rough)"
                        } else {
                            ""
                        }
                    ))
                )
            })
            .collect();
        let placed = output
            .accounts
            .iter()
            .filter(|other| other.agent == account.agent && other.place.is_some())
            .count();
        lines.push(format!(
            "{} / {}{}{}: {} · {} · {}{}",
            account.agent.word(),
            if account.name.is_empty() {
                &account.id
            } else {
                &account.name
            },
            if account.current { " (current)" } else { "" },
            if account.you { " (you)" } else { "" },
            match account.place {
                None => account.why.clone(),
                Some(_) if placed == 1 => "the one account that takes work".into(),
                Some(place) => format!("takes work {} ({})", ordinal(place), account.why),
            },
            account.fit,
            if usage.is_empty() {
                "usage unknown".into()
            } else {
                usage.join("; ")
            },
            read_ago(account.age_s)
        ));
    }
    lines.push("Guidance:".into());
    for g in &output.guidance {
        lines.push(format!(
            "{}: {} / {} / {} — {} ({})",
            g.work,
            g.agent.word(),
            g.model,
            g.effort.as_deref().unwrap_or("default"),
            g.reason,
            g.source
        ));
    }
    lines.push("Your rules (~/.brigadier/models.md):".into());
    for rule in &output.rules {
        lines.push(format!("  {}", rule.rule));
    }
    if output.rules.is_empty() {
        lines.push("  None.".into());
    }
    if lines.len() > 25 {
        lines.truncate(24);
        lines.push("More rows: brigadier models --json".into());
    }
    lines.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Account, ModelsOutput, Usage};
    use crate::protocol::Agent;

    #[test]
    fn why_names_the_turn_its_message_reply_and_ledger() {
        use crate::protocol::{WhyLedgerEntry, WhySession, WhyTurn};
        let output = WhyOutput {
            path: "src/a.rs".into(),
            line: 4,
            text: "    let x = 1;".into(),
            by: WhyBy::Turn,
            session: Some(WhySession {
                id: "a1b2c3d4".into(),
                title: "Parser".into(),
                agent: crate::protocol::Agent::Claude,
            }),
            turn: Some(WhyTurn {
                number: 2,
                started_at_ms: None,
                ended_at_ms: None,
                path: "src/a.rs".into(),
                line: 3,
                message: Some("Add x.\nKeep it short.".into()),
                reply: Some("Added x.".into()),
                reply_cut: true,
                read: Some("brigadier brain read a1b2c3d4 7 9".into()),
                ledger: vec![WhyLedgerEntry {
                    id: Some("L4".into()),
                    text: "- L4 · 10-24 09:12 · gen 1 · Decision: x starts at 1".into(),
                }],
                gone: None,
            }),
            worker: None,
            commit: None,
            moved_by: Vec::new(),
        };
        assert_eq!(
            why(&output),
            "src/a.rs:4  let x = 1;\n\
             Written in turn 2 of session a1b2c3d4 \"Parser\" (claude).\n\
             \nMessage:\n  Add x.\n  Keep it short.\n\
             \nReply (its start; whole: brigadier brain read a1b2c3d4 7 9):\n  Added x.\n\
             \nLedger, written in that turn:\n  - L4 · 10-24 09:12 · gen 1 · Decision: x starts at 1\n"
        );
        let outside = WhyOutput {
            by: WhyBy::Uncommitted,
            session: None,
            turn: None,
            ..output
        };
        assert!(
            why(&outside).contains("isn't committed"),
            "{}",
            why(&outside)
        );
    }

    #[test]
    fn models_reads_window_ids_as_words_and_times_as_times_not_epoch_ms() {
        let output = ModelsOutput {
            workers_at_once: 4,
            models: vec![],
            agents: vec![],
            switch_providers: false,
            accounts: vec![Account {
                id: "own".into(),
                name: "own".into(),
                agent: Agent::Claude,
                current: true,
                you: true,
                logged_in: true,
                age_s: Some(40),
                fit: "ok".into(),
                usage: vec![Usage {
                    window: "five_hour".into(),
                    used_percent: 8.0,
                    resets_at_ms: Some(1_792_783_200_000),
                    model: None,
                    bucket: None,
                    pooled: true,
                    runway_ms: None,
                    estimate_rough: false,
                }],
                place: Some(1),
                why: "last".into(),
            }],
            guidance: vec![],
            rules: vec![],
        };
        let text = models(&output);
        // The worker count leads, so the 25-line cut never hides it (run2-D31).
        assert_eq!(
            text.lines().nth(1),
            Some("Workers at once: up to 4 (the user's setting, Settings › Configuration)")
        );
        assert!(!text.contains("five_hour"), "{text}");
        assert!(!text.contains("1792783200000"), "{text}");
        assert!(text.contains("5-hour 8% used, resets "), "{text}");
        // One account: no "#1 last", and the figure's age in words.
        assert!(text.contains("the one account that takes work"), "{text}");
        assert!(text.contains("read 40s ago"), "{text}");
        assert!(!text.contains("#1"), "{text}");
        assert_eq!(ordinal(1), "1st");
        assert_eq!(ordinal(12), "12th");
        assert_eq!(ordinal(22), "22nd");
    }
}
