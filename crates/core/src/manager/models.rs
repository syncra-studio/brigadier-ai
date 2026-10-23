//! Model advice and the user's editable policy. Only directives in the user's file gate a start.
use super::SessionManager;
use crate::{Error, Result, now_ms};
use crate::{GuidanceRow, ModelRule, ModelRules};
use brigadier_cli::models::{Account, Guidance, Model, ModelsOutput, Rule, Usage};
use brigadier_cli::protocol::Agent;
use brigadier_providers::{AllowedModels, ProviderKind};
use std::path::PathBuf;

const GUIDANCE: &str = include_str!("models_guidance.md");
pub(super) const DEFAULT_RULES: &str = include_str!("models_default.md");

fn agent(provider: ProviderKind) -> Agent {
    match provider {
        ProviderKind::Claude => Agent::Claude,
        ProviderKind::Codex => Agent::Codex,
    }
}
/// An agent as the rules file names it.
fn word(provider: ProviderKind) -> &'static str {
    match provider {
        ProviderKind::Claude => "claude",
        ProviderKind::Codex => "codex",
    }
}
fn rank(effort: &str) -> Option<usize> {
    [
        "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
    ]
    .iter()
    .position(|s| effort.eq_ignore_ascii_case(s))
}
fn matches(pattern: &str, model: &str) -> bool {
    pattern == "*"
        || model.eq_ignore_ascii_case(pattern)
        || model
            .split(|c: char| !c.is_alphanumeric())
            .any(|word| word.eq_ignore_ascii_case(pattern))
}
fn rows(text: &str) -> Vec<GuidanceRow> {
    text.lines()
        .filter_map(|line| {
            let cells: Vec<_> = line
                .trim()
                .trim_matches('|')
                .split('|')
                .map(str::trim)
                .collect();
            if cells.len() != 5 {
                return None;
            }
            let agent = match cells[1] {
                "claude" => ProviderKind::Claude,
                "codex" => ProviderKind::Codex,
                _ => return None,
            };
            Some(GuidanceRow {
                work: cells[0].into(),
                agent,
                model: cells[2].into(),
                effort: (!cells[3].is_empty()).then(|| cells[3].into()),
                reason: cells[4].into(),
            })
        })
        .collect()
}
fn guidance(shipped: &str, user: &str) -> Vec<Guidance> {
    let shown = |row: GuidanceRow, source: &str| Guidance {
        work: row.work,
        agent: agent(row.agent),
        model: row.model,
        effort: row.effort,
        reason: row.reason,
        source: source.into(),
    };
    let mut merged: Vec<_> = rows(shipped)
        .into_iter()
        .map(|r| shown(r, "shipped"))
        .collect();
    for row in rows(user) {
        let row = shown(row, "user");
        if let Some(old) = merged.iter_mut().find(|old| old.work == row.work) {
            *old = row;
        } else {
            merged.push(row);
        }
    }
    merged
}
/// The directive a rules-file line holds, with the line as the user wrote it; `None` for prose.
fn directive(line: &str) -> Result<Option<(String, ModelRule)>> {
    let rule = line.trim().strip_prefix("- ").unwrap_or(line.trim());
    let Some((key, value)) = rule.split_once(':') else {
        return Ok(None);
    };
    let value = value.trim();
    let names = || {
        value
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    let parsed = match key.trim() {
        "forbid" => ModelRule::Forbid { names: names() },
        "allow" => ModelRule::Allow {
            agent: None,
            names: names(),
        },
        "allow claude" => ModelRule::Allow {
            agent: Some(ProviderKind::Claude),
            names: names(),
        },
        "allow codex" => ModelRule::Allow {
            agent: Some(ProviderKind::Codex),
            names: names(),
        },
        "effort-ceiling" => {
            rank(value).ok_or_else(|| Error::Invalid(format!("invalid model rule: {rule}")))?;
            ModelRule::EffortCeiling {
                effort: value.to_ascii_lowercase(),
            }
        }
        _ => return Ok(None),
    };
    Ok(Some((rule.into(), parsed)))
}
pub(super) struct Policy {
    entries: Vec<(String, ModelRule)>,
}
impl Policy {
    fn parse(text: &str) -> Result<Self> {
        let mut entries = Vec::new();
        for line in text.lines() {
            entries.extend(directive(line)?);
        }
        Ok(Self { entries })
    }
    fn refusal(
        &self,
        provider: ProviderKind,
        models: &[&str],
        effort: Option<&str>,
    ) -> Option<String> {
        self.entries.iter().find_map(|(rule, directive)| {
            let violates = match directive {
                ModelRule::Forbid { names } => {
                    models.iter().any(|m| names.iter().any(|p| matches(p, m)))
                }
                ModelRule::Allow { agent, names } => {
                    agent.is_none_or(|p| p == provider)
                        && !models.is_empty()
                        && !models.iter().any(|m| names.iter().any(|p| matches(p, m)))
                }
                ModelRule::EffortCeiling { effort: ceiling } => effort
                    .is_some_and(|e| rank(e).is_none_or(|r| rank(ceiling).is_some_and(|c| r > c))),
            };
            violates.then(|| format!("refused by your model rule: \"{rule}\""))
        })
    }
    fn restricts_models(&self, provider: ProviderKind) -> bool {
        self.entries.iter().any(|(_, d)| match d {
            ModelRule::Forbid { names } => !names.is_empty(),
            ModelRule::Allow { agent, .. } => agent.is_none_or(|p| p == provider),
            ModelRule::EffortCeiling { .. } => false,
        })
    }
}
/// A rule as the file writes it: `forbid: fable`, `allow claude: opus, sonnet`.
fn rule_text(rule: &ModelRule) -> String {
    match rule {
        ModelRule::Forbid { names } => format!("forbid: {}", names.join(", ")),
        ModelRule::Allow { agent, names } => format!(
            "allow{}: {}",
            agent.map_or(String::new(), |p| format!(" {}", word(p))),
            names.join(", ")
        ),
        ModelRule::EffortCeiling { effort } => format!("effort-ceiling: {effort}"),
    }
}
const TABLE_HEAD: &str =
    "| Work | Agent | Model | Effort | Reason |\n| --- | --- | --- | --- | --- |";
/// Checks what Settings › Routing sends before it reaches the file: every cell fits the file's
/// one-line table, names fit a rule, efforts are levels, and no work has two rows.
fn check_rules(rows: &[GuidanceRow], rules: &[ModelRule]) -> Result<()> {
    let bad = |what: &str| Err(Error::Invalid(format!("invalid model rules: {what}")));
    let cell = |text: &str| text.contains(['|', '\n', '\r']);
    for (i, row) in rows.iter().enumerate() {
        if row.work.trim().is_empty() || row.model.trim().is_empty() {
            return bad("a row needs a work and a model");
        }
        if [&row.work, &row.model, &row.reason]
            .into_iter()
            .chain(&row.effort)
            .any(|text| cell(text))
        {
            return bad("a row's cells cannot hold | or a line break");
        }
        if row.effort.as_deref().is_some_and(|e| rank(e).is_none()) {
            return bad("unknown effort level");
        }
        if rows[..i].iter().any(|other| other.work == row.work) {
            return bad(&format!("two rows for \"{}\"", row.work));
        }
    }
    for rule in rules {
        match rule {
            ModelRule::Forbid { names } | ModelRule::Allow { names, .. } => {
                if names.is_empty()
                    || names
                        .iter()
                        .any(|n| n.trim().is_empty() || n.contains([',', ':', '\n', '\r']))
                {
                    return bad("a rule names models without commas, colons or line breaks");
                }
            }
            ModelRule::EffortCeiling { effort } => {
                if rank(effort).is_none() {
                    return bad("unknown effort level");
                }
            }
        }
    }
    Ok(())
}
/// The rules file with its rows and rules replaced, in the same text format: prose and comments
/// stay where they are, the rules take the place of the first rule line and the table the place
/// of the first table line (else each goes at the end).
fn rewrite(text: &str, rows: &[GuidanceRow], rules: &[ModelRule]) -> String {
    let rules_block: Vec<String> = rules
        .iter()
        .map(|r| format!("- {}", rule_text(r)))
        .collect();
    let mut table = vec![TABLE_HEAD.to_owned()];
    table.extend(rows.iter().map(|r| {
        format!(
            "| {} | {} | {} | {} | {} |",
            r.work.trim(),
            word(r.agent),
            r.model.trim(),
            r.effort.as_deref().unwrap_or("").trim(),
            r.reason.trim()
        )
    }));
    let (mut rules_done, mut table_done) = (false, false);
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        if line.trim_start().starts_with('|') {
            if !table_done && !rows.is_empty() {
                out.extend(table.iter().cloned());
            }
            table_done = true;
        } else if directive(line).ok().flatten().is_some() {
            if !rules_done {
                out.extend(rules_block.iter().cloned());
            }
            rules_done = true;
        } else {
            out.push(line.to_owned());
        }
    }
    for (done, block, wanted) in [
        (rules_done, rules_block, !rules.is_empty()),
        (table_done, table, !rows.is_empty()),
    ] {
        if !done && wanted {
            while out.last().is_some_and(|l| l.trim().is_empty()) {
                out.pop();
            }
            if !out.is_empty() {
                out.push(String::new());
            }
            out.extend(block);
        }
    }
    let mut written = out.join("\n");
    written.push('\n');
    written
}
/// Creates the shipped copy once, preserving an existing file, including an empty one.
fn editable(path: &std::path::Path, default: &str) -> Result<String> {
    if !path.exists() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| Error::Invalid(format!("{}: {e}", path.display())))?;
        }
        use std::io::Write;
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
        {
            Ok(mut file) => file
                .write_all(default.as_bytes())
                .map_err(|e| Error::Invalid(format!("{}: {e}", path.display())))?,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(Error::Invalid(format!("{}: {e}", path.display()))),
        }
    }
    std::fs::read_to_string(path).map_err(|e| Error::Invalid(format!("{}: {e}", path.display())))
}
impl SessionManager {
    pub(super) fn models_rules_path(&self) -> PathBuf {
        if cfg!(test) {
            return self.data_dir.join("models.md");
        }
        std::env::var_os("BRIGADIER_MODELS_FILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::home_dir()
                    .unwrap_or_else(|| self.data_dir.clone())
                    .join(".brigadier/models.md")
            })
    }
    /// Settings › Routing's read of the rules file, as it is on disk now (S32).
    pub fn model_rules(&self) -> Result<ModelRules> {
        let path = self.models_rules_path();
        let text = editable(&path, DEFAULT_RULES)?;
        Ok(ModelRules {
            path: path.display().to_string(),
            shipped: rows(GUIDANCE),
            rows: rows(&text),
            rules: Policy::parse(&text)?
                .entries
                .into_iter()
                .map(|(_, rule)| rule)
                .collect(),
        })
    }
    /// Settings › Routing's save: the file's rows and rules become these, in the file's own text
    /// format, its prose kept (S32).
    pub fn set_model_rules(&self, rows: &[GuidanceRow], rules: &[ModelRule]) -> Result<ModelRules> {
        check_rules(rows, rules)?;
        let path = self.models_rules_path();
        let text = rewrite(&editable(&path, DEFAULT_RULES)?, rows, rules);
        let io = |e: std::io::Error| Error::Invalid(format!("{}: {e}", path.display()));
        let staged = path.with_extension("md.saving");
        std::fs::write(&staged, text).map_err(io)?;
        std::fs::rename(&staged, &path).map_err(io)?;
        self.model_rules()
    }
    /// `brigadier start --work`'s row: the user's row of that work, else the shipped one (S32).
    pub(super) fn work_row(&self, work: &str) -> Result<Option<Guidance>> {
        let user = editable(&self.models_rules_path(), DEFAULT_RULES)?;
        Ok(guidance(GUIDANCE, &user)
            .into_iter()
            .find(|row| row.work.trim().eq_ignore_ascii_case(work.trim())))
    }
    pub(super) fn model_policy(&self) -> Result<Policy> {
        Policy::parse(&editable(&self.models_rules_path(), DEFAULT_RULES)?)
    }
    pub(super) fn model_refusal(
        &self,
        provider: ProviderKind,
        model: Option<&str>,
        effort: Option<&str>,
    ) -> Result<Option<String>> {
        let catalog = self.runtime.overview(provider).and_then(|o| o.models);
        let selected = catalog.as_ref().and_then(|c| {
            c.models.iter().find(|m| {
                model.map_or(m.is_default, |id| {
                    m.id.eq_ignore_ascii_case(id)
                        || m.resolved
                            .as_deref()
                            .is_some_and(|r| r.eq_ignore_ascii_case(id))
                })
            })
        });
        let names: Vec<_> = model
            .into_iter()
            .chain(selected.map(|m| m.id.as_str()))
            .chain(selected.and_then(|m| m.resolved.as_deref()))
            .collect();
        let effort = effort.or(selected.and_then(|m| m.default_effort.as_deref()));
        Ok(self.model_policy()?.refusal(provider, &names, effort))
    }
    pub(super) fn rules_models(&self, provider: ProviderKind) -> Result<Option<AllowedModels>> {
        let policy = self.model_policy()?;
        if !policy.restricts_models(provider) {
            return Ok(None);
        }
        let mut allowed = AllowedModels::default();
        for model in self
            .runtime
            .overview(provider)
            .and_then(|o| o.models)
            .into_iter()
            .flat_map(|c| c.models)
        {
            let names: Vec<_> = std::iter::once(model.id.as_str())
                .chain(model.resolved.as_deref())
                .collect();
            let denied = policy.refusal(provider, &names, None).is_some();
            for id in [Some(model.id), model.resolved].into_iter().flatten() {
                let list = if denied {
                    &mut allowed.outside
                } else {
                    &mut allowed.ids
                };
                if !list.contains(&id) {
                    list.push(id);
                }
            }
        }
        // A deny rule that matches none of this CLI's models adds no allowlist. In particular,
        // a rule for a different provider cannot disable this CLI's native subagents.
        if !allowed.ids.is_empty() && allowed.outside.is_empty() && !policy.entries.iter().any(|(_, d)| matches!(d, ModelRule::Allow { agent, .. } if agent.is_none_or(|p| p == provider))) {
            return Ok(None);
        }
        Ok(Some(allowed))
    }
    pub(super) async fn cli_models(&self, caller: &super::cli::CliCaller) -> Result<ModelsOutput> {
        let now = now_ms();
        let user = editable(&self.models_rules_path(), DEFAULT_RULES)?;
        let policy = Policy::parse(&user)?;
        let shipped = GUIDANCE;
        let mut models = Vec::new();
        for provider in ProviderKind::ALL {
            if let Some(catalog) = self.runtime.overview(provider).and_then(|o| o.models) {
                let age_s = Some((now - catalog.fetched_at_ms).max(0) / 1000);
                for model in catalog.models {
                    models.push(Model {
                        agent: agent(provider),
                        model: model.id,
                        available: true,
                        efforts: model.efforts,
                        age_s,
                    });
                }
            }
        }
        let current = self.convs_lock().get(&caller.conversation_id).cloned();
        let current = if let Some(task) = &caller.task_id {
            let live = self.tasks_lock().get(task).cloned();
            if let Some(live) = live {
                live.cli().await
            } else {
                None
            }
        } else if let Some(conv) = current {
            conv.cli().await
        } else {
            None
        };
        let accounts = self
            .runtime
            .accounts_view()
            .accounts
            .into_iter()
            .map(|a| {
                let quota = self.runtime.account_usage(&a.account, now);
                let usage: Vec<_> = quota
                    .as_ref()
                    .into_iter()
                    .flat_map(|q| &q.windows)
                    .map(|w| Usage {
                        window: w.window.id.clone(),
                        used_percent: w.window.used_percent,
                        resets_at_ms: w.window.resets_at_ms,
                        model: w.window.model.clone(),
                        bucket: w.window.bucket.clone(),
                        pooled: w.window.model.is_none(),
                        runway_ms: w
                            .forecast
                            .and_then(|f| f.runs_out_at_ms)
                            .map(|at| (at - now).max(0)),
                        estimate_rough: w.forecast.is_some_and(|f| f.gap),
                    })
                    .collect();
                let fit = fit(
                    &usage,
                    quota
                        .as_ref()
                        .and_then(|q| q.limit.as_ref())
                        .map(|l| l.resets_at_ms),
                );
                Account {
                    id: a.account.account.clone().unwrap_or_else(|| "own".into()),
                    name: a.name,
                    agent: agent(a.account.provider),
                    current: a.in_use,
                    you: current.as_ref().is_some_and(|cli| cli.account == a.account),
                    logged_in: a.status.is_some_and(|s| s.logged_in),
                    age_s: a.quota_at_ms.map(|at| (now - at).max(0) / 1000),
                    fit,
                    usage,
                }
            })
            .collect();
        Ok(ModelsOutput {
            models,
            accounts,
            guidance: guidance(shipped, &user),
            rules: policy
                .entries
                .into_iter()
                .map(|(rule, _)| Rule { rule })
                .collect(),
        })
    }
}
fn fit(windows: &[Usage], limited: Option<Option<i64>>) -> String {
    if matches!(limited, Some(None)) {
        return "out".into();
    }
    if let Some(at) = limited.flatten().or_else(|| {
        windows
            .iter()
            .filter(|w| w.pooled && w.used_percent >= 97.0)
            .filter_map(|w| w.resets_at_ms)
            .max()
    }) {
        return format!(
            "out until {:02}:{:02} UTC",
            (at / 3_600_000).rem_euclid(24),
            (at / 60_000).rem_euclid(60)
        );
    }
    if windows.iter().any(|w| w.pooled && (w.used_percent >= 97.0)) {
        "out".into()
    } else if windows
        .iter()
        .any(|w| w.pooled && (w.used_percent >= 85.0 || w.runway_ms.is_some()))
    {
        "tight".into()
    } else if windows.is_empty() {
        "unknown".into()
    } else {
        "ok".into()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn user_row_overrides_shipped_row() {
        let rows = guidance(
            GUIDANCE,
            "| Bounded implementation | claude | sonnet | low | My preference. |",
        );
        let row = rows
            .iter()
            .find(|r| r.work == "Bounded implementation")
            .unwrap();
        assert_eq!(row.model, "sonnet");
        assert_eq!(row.source, "user");
    }
    #[test]
    fn shipped_default_has_no_rules() {
        let policy = Policy::parse(DEFAULT_RULES).unwrap();
        assert!(policy.entries.is_empty());
        for provider in ProviderKind::ALL {
            assert!(!policy.restricts_models(provider));
            assert!(
                policy
                    .refusal(provider, &["claude-fable-5-1", "any-model"], Some("ultra"))
                    .is_none()
            );
        }
    }
    #[test]
    fn user_rules_are_displayed_and_quote_the_rule() {
        let policy = Policy::parse("- forbid: fable\n- effort-ceiling: high\n").unwrap();
        assert_eq!(policy.entries[0].0, "forbid: fable");
        assert!(
            policy
                .refusal(ProviderKind::Claude, &["claude-fable-5-1"], None)
                .unwrap()
                .contains("forbid: fable")
        );
        assert!(
            Policy::parse("")
                .unwrap()
                .refusal(ProviderKind::Claude, &["claude-fable-5-1"], Some("ultra"))
                .is_none()
        );
        assert!(
            policy
                .refusal(ProviderKind::Codex, &[], Some("xhigh"))
                .unwrap()
                .contains("effort-ceiling: high")
        );
        let policy = Policy::parse("- allow claude: opus, sonnet").unwrap();
        assert!(
            policy
                .refusal(ProviderKind::Codex, &["other"], None)
                .is_none()
        );
        assert!(
            policy
                .refusal(ProviderKind::Claude, &["haiku"], None)
                .is_some()
        );
    }
    #[test]
    fn allowlists_accept_a_reported_alias_and_denials_inspect_its_resolution() {
        let names = ["opus", "claude-opus-5-5"];
        assert!(
            Policy::parse("- allow claude: claude-opus-5-5")
                .unwrap()
                .refusal(ProviderKind::Claude, &names, None)
                .is_none()
        );
        assert!(
            Policy::parse("- forbid: fable")
                .unwrap()
                .refusal(ProviderKind::Claude, &["opus", "claude-fable-5-1"], None)
                .is_some()
        );
    }
    #[test]
    fn the_page_writes_the_files_own_format_and_keeps_its_prose() {
        let row = GuidanceRow {
            work: "Bounded implementation".into(),
            agent: ProviderKind::Claude,
            model: "sonnet".into(),
            effort: Some("medium".into()),
            reason: "My preference.".into(),
        };
        let rules = vec![
            ModelRule::Forbid {
                names: vec!["fable".into()],
            },
            ModelRule::Allow {
                agent: Some(ProviderKind::Codex),
                names: vec!["gpt-6".into(), "default".into()],
            },
            ModelRule::EffortCeiling {
                effort: "high".into(),
            },
        ];
        check_rules(std::slice::from_ref(&row), &rules).unwrap();
        // The shipped file has prose only: rules and the table go after it.
        let written = rewrite(DEFAULT_RULES, std::slice::from_ref(&row), &rules);
        assert!(written.starts_with(DEFAULT_RULES.trim_end()));
        assert!(written.contains(
            "- forbid: fable\n- allow codex: gpt-6, default\n- effort-ceiling: high\n\n| Work |"
        ));
        assert!(
            written.ends_with(
                "| Bounded implementation | claude | sonnet | medium | My preference. |\n"
            )
        );
        // Read back, it is what was written; written again, it is the same text.
        let policy = Policy::parse(&written).unwrap();
        assert_eq!(
            policy
                .entries
                .into_iter()
                .map(|(_, r)| r)
                .collect::<Vec<_>>(),
            rules
        );
        assert_eq!(rows(&written), vec![row.clone()]);
        assert_eq!(
            rewrite(&written, std::slice::from_ref(&row), &rules),
            written
        );
        // A hand edit in place: a rule between notes keeps its place; one removed leaves the rest.
        let hand = "# Mine\nnote one\n- forbid: mythos\nnote two\n| Work | Agent | Model | Effort | Reason |\n| --- | --- | --- | --- | --- |\n| Review | codex | default | high | Mine. |\n";
        let edited = rewrite(hand, &[], &rules[..1]);
        assert_eq!(edited, "# Mine\nnote one\n- forbid: fable\nnote two\n");
        // The page never writes what the file cannot hold.
        let mut bad = row.clone();
        bad.reason = "a | b".into();
        assert!(check_rules(&[bad], &[]).is_err());
        assert!(check_rules(&[row.clone(), row], &[]).is_err());
        assert!(check_rules(&[], &[ModelRule::Forbid { names: vec![] }]).is_err());
        assert!(
            check_rules(
                &[],
                &[ModelRule::EffortCeiling {
                    effort: "huge".into()
                }]
            )
            .is_err()
        );
    }
    #[test]
    fn fit_and_runway_respect_pooled_and_scoped_windows() {
        let mut window = Usage {
            window: "primary".into(),
            used_percent: 20.0,
            resets_at_ms: Some(3_600_000),
            model: None,
            bucket: None,
            pooled: true,
            runway_ms: None,
            estimate_rough: false,
        };
        assert_eq!(fit(&[], None), "unknown");
        assert_eq!(fit(&[], Some(None)), "out");
        assert_eq!(fit(&[window.clone()], None), "ok");
        window.runway_ms = Some(1000);
        assert_eq!(fit(&[window.clone()], None), "tight");
        window.used_percent = 100.0;
        assert_eq!(fit(&[window.clone()], None), "out until 01:00 UTC");
        window.pooled = false;
        assert_eq!(
            fit(&[window.clone()], None),
            "ok",
            "a scoped window does not exhaust every model"
        );
        assert_eq!(fit(&[window], Some(Some(3_600_000))), "out until 01:00 UTC");
    }
}
