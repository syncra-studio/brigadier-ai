//! Model advice and the user's editable policy. Only directives in the user's file gate a start.
use super::SessionManager;
use crate::{Error, Result, now_ms};
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
fn rows(text: &str, source: &str) -> Vec<Guidance> {
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
                "claude" => Agent::Claude,
                "codex" => Agent::Codex,
                _ => return None,
            };
            Some(Guidance {
                work: cells[0].into(),
                agent,
                model: cells[2].into(),
                effort: (!cells[3].is_empty()).then(|| cells[3].into()),
                reason: cells[4].into(),
                source: source.into(),
            })
        })
        .collect()
}
fn guidance(shipped: &str, user: &str) -> Vec<Guidance> {
    let mut merged = rows(shipped, "shipped");
    for row in rows(user, "user") {
        if let Some(old) = merged.iter_mut().find(|old| old.work == row.work) {
            *old = row;
        } else {
            merged.push(row);
        }
    }
    merged
}
#[derive(Clone)]
enum Directive {
    Forbid(Vec<String>),
    Allow(Option<ProviderKind>, Vec<String>),
    Ceiling(usize),
}
pub(super) struct Policy {
    entries: Vec<(String, Directive)>,
}
impl Policy {
    fn parse(text: &str) -> Result<Self> {
        let mut entries = Vec::new();
        for line in text.lines() {
            let rule = line.trim().strip_prefix("- ").unwrap_or(line.trim());
            let Some((key, value)) = rule.split_once(':') else {
                continue;
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
            let directive = match key.trim() {
                "forbid" => Directive::Forbid(names()),
                "allow" => Directive::Allow(None, names()),
                "allow claude" => Directive::Allow(Some(ProviderKind::Claude), names()),
                "allow codex" => Directive::Allow(Some(ProviderKind::Codex), names()),
                "effort-ceiling" => Directive::Ceiling(
                    rank(value)
                        .ok_or_else(|| Error::Invalid(format!("invalid model rule: {rule}")))?,
                ),
                _ => continue,
            };
            entries.push((rule.into(), directive));
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
                Directive::Forbid(names) => {
                    models.iter().any(|m| names.iter().any(|p| matches(p, m)))
                }
                Directive::Allow(scope, names) => {
                    scope.is_none_or(|p| p == provider)
                        && !models.is_empty()
                        && !models.iter().any(|m| names.iter().any(|p| matches(p, m)))
                }
                Directive::Ceiling(ceiling) => {
                    effort.is_some_and(|e| rank(e).is_none_or(|r| r > *ceiling))
                }
            };
            violates.then(|| format!("refused by your model rule: \"{rule}\""))
        })
    }
    fn restricts_models(&self, provider: ProviderKind) -> bool {
        self.entries.iter().any(|(_, d)| match d {
            Directive::Forbid(names) => !names.is_empty(),
            Directive::Allow(scope, _) => scope.is_none_or(|p| p == provider),
            Directive::Ceiling(_) => false,
        })
    }
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
        if !allowed.ids.is_empty() && allowed.outside.is_empty() && !policy.entries.iter().any(|(_, d)| matches!(d, Directive::Allow(scope, _) if scope.is_none_or(|p| p == provider))) {
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
