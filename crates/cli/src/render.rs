//! The text output of each command (`--json` prints the reply's output as is).

use base64::Engine as _;

use crate::protocol::{
    Answer, AskOutput, Changes, CloseOutput, Item, ItemKind, ReadOutput, SendOutput,
    SessionsOutput, ShowOutput, StartOutput, ToolOutput, WaitOutput, WorkerRow, WorkersOutput,
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
    let mut rows = vec![vec![
        "ID".into(),
        "STATE".into(),
        "CHANGES".into(),
        "WORKERS".into(),
        "BRANCH".into(),
        "TITLE".into(),
    ]];
    for session in &output.sessions {
        let title = if session.caller {
            format!("{} (this session)", session.title)
        } else {
            session.title.clone()
        };
        rows.push(vec![
            session.id.clone(),
            session.state.word().into(),
            changes(session.changes),
            session.workers.to_string(),
            session.branch.clone().unwrap_or_else(|| "-".into()),
            title,
        ]);
    }
    out.push_str(&table(&rows));
    out
}

fn worker_rows(workers: &[WorkerRow]) -> String {
    let mut rows = vec![vec![
        "ID".into(),
        "#".into(),
        "STATE".into(),
        "AGENT".into(),
        "MODEL".into(),
        "CHANGES".into(),
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
        None if output.queued => format!("Queued into {}'s turn.\n", output.id),
        None => format!("Sent; {} works on it.\n", output.id),
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

pub fn close(output: &CloseOutput) -> String {
    if output.closed {
        format!("Closed {}.\n", output.id)
    } else {
        format!("{} had already ended.\n", output.id)
    }
}

/// `preview`, `browser`, `app` and `page`: the tool's answer, and where its screenshots went.
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

/// A compact advice view. `--json` carries every row when the terminal summary is abbreviated.
pub fn models(output: &crate::models::ModelsOutput) -> String {
    let mut lines = vec!["Models and usage (advice; only your rules restrict starts)".to_owned()];
    for agent in [
        crate::protocol::Agent::Claude,
        crate::protocol::Agent::Codex,
    ] {
        let models: Vec<_> = output
            .models
            .iter()
            .filter(|m| m.agent == agent)
            .map(|m| {
                format!(
                    "{} [{}] age {}s",
                    m.model,
                    m.efforts.join("/"),
                    m.age_s.map_or_else(|| "?".into(), |age| age.to_string())
                )
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
                    "{} {}% reset {}{}{}",
                    w.window,
                    w.used_percent,
                    w.resets_at_ms
                        .map_or_else(|| "unknown".into(), |at| at.to_string()),
                    if w.pooled { " pooled" } else { "" },
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
        lines.push(format!(
            "{} / {}{}{}: {} · {} · {} · age {}s",
            account.agent.word(),
            if account.name.is_empty() {
                &account.id
            } else {
                &account.name
            },
            if account.current { " (current)" } else { "" },
            if account.you { " (you)" } else { "" },
            account.place.map_or_else(
                || account.why.clone(),
                |place| format!("#{place} {}", account.why)
            ),
            account.fit,
            if usage.is_empty() {
                "usage unknown".into()
            } else {
                usage.join("; ")
            },
            account
                .age_s
                .map_or_else(|| "?".into(), |age| age.to_string())
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
