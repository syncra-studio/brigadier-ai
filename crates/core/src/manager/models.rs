//! Model advice and the user's editable policy. Only directives in the user's file gate a start.
use super::SessionManager;
use crate::{Error, Result, now_ms};
use crate::{GuidanceRow, ModelRule, ModelRules};
use brigadier_cli::models::{Account, AgentAccounts, Guidance, Model, ModelsOutput, Rule, Usage};
use brigadier_providers::{AllowedModels, KindFacts, ProviderKind};
use brigadier_router::{MergedModel, PickRule, Rating};
use std::path::PathBuf;

const GUIDANCE: &str = include_str!("models_guidance.md");
pub(super) const DEFAULT_RULES: &str = include_str!("models_default.md");
/// The rules file's Work for a session that goes on with another agent (S38).
/// The Work whose rows a session moves to another agent with (S38, D25, D34).
pub(crate) const FALLBACK_WORK: &str = "Fallback";

/// Read the former work name in existing rows, scoped rules and `--work` starts.
pub(super) fn canonical_work(work: &str) -> &str {
    let work = work.trim();
    if work.eq_ignore_ascii_case("Independent review") {
        "Adversarial review"
    } else {
        work
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
fn table_cells(line: &str) -> Vec<&str> {
    line.trim()
        .trim_matches('|')
        .split('|')
        .map(str::trim)
        .collect()
}
fn routing_header(line: &str) -> bool {
    line.trim_start().starts_with('|')
        && table_cells(line) == ["Work", "Agent", "Model", "Effort", "Reason"]
}
fn table_separator(line: &str) -> bool {
    line.trim_start().starts_with('|')
        && table_cells(line).iter().all(|cell| {
            let dashes = cell.trim_matches(':');
            dashes.len() >= 3 && dashes.chars().all(|c| c == '-')
        })
}
/// The readable routing rows and their line numbers. Old files may have rows without a header;
/// a table with another header is prose, even if a cell happens to name an agent.
fn routing_rows(text: &str) -> Vec<(usize, GuidanceRow)> {
    let mut lines = text.lines().enumerate().peekable();
    let mut routing = true;
    let mut found = Vec::new();
    while let Some((index, line)) = lines.next() {
        if !line.trim_start().starts_with('|') {
            routing = true;
            continue;
        }
        if lines.peek().is_some_and(|(_, next)| table_separator(next)) {
            routing = routing_header(line);
            lines.next();
            continue;
        }
        if !routing {
            continue;
        }
        let cells = table_cells(line);
        if cells.len() != 5 {
            continue;
        }
        let Some(agent) = agent_named(cells[1]) else {
            continue;
        };
        found.push((
            index,
            GuidanceRow {
                work: canonical_work(cells[0]).into(),
                agent,
                model: cells[2].into(),
                effort: (!cells[3].is_empty()).then(|| cells[3].into()),
                reason: cells[4].into(),
            },
        ));
    }
    found
}
fn rows(text: &str) -> Vec<GuidanceRow> {
    routing_rows(text).into_iter().map(|(_, row)| row).collect()
}
/// The shipped guidance's rows.
#[cfg(test)]
pub(super) fn shipped_rows() -> Vec<GuidanceRow> {
    rows(GUIDANCE)
}
fn guidance(shipped: Vec<GuidanceRow>, user: &str) -> Vec<Guidance> {
    let shown = |row: GuidanceRow, source: &str| Guidance {
        work: row.work,
        agent: row.agent,
        model: row.model,
        effort: row.effort,
        reason: row.reason,
        source: source.into(),
    };
    // A work's rows of the user's (an order, first to last) replace all its shipped rows, in
    // the place of the first; a work of the user's own goes after the shipped ones.
    let user = rows(user);
    let mut merged: Vec<Guidance> = Vec::new();
    for row in &shipped {
        let mut mine = user
            .iter()
            .filter(|mine| same_work(&mine.work, &row.work))
            .peekable();
        if mine.peek().is_none() {
            let source = if row.reason.starts_with(RATED_REASON) {
                "ratings"
            } else {
                "shipped"
            };
            merged.push(shown(row.clone(), source));
        } else if !merged.iter().any(|old| same_work(&old.work, &row.work)) {
            merged.extend(mine.map(|mine| shown(mine.clone(), "user")));
        }
    }
    for row in &user {
        if !shipped.iter().any(|old| same_work(&old.work, &row.work)) {
            merged.push(shown(row.clone(), "user"));
        }
    }
    merged
}
/// An agent as the rules file names it: a CLI Brigadier drives.
fn agent_named(word: &str) -> Option<ProviderKind> {
    ProviderKind::parse(word)
        .filter(|agent| brigadier_providers::adapters::adapter(*agent).is_some())
}
/// How a shipped kind of work picks its automatic models from the ratings, and the one agent a
/// fusion consult's kind is for (brief L11). `None`: the kind keeps its shipped rows.
fn pick_rule(work: &str) -> Option<(PickRule, Option<ProviderKind>)> {
    use brigadier_router::TaskCategory as C;
    let rule = |rating, small, effort_from| PickRule {
        rating,
        small,
        effort_from,
        fallback_effort: small.then_some("low"),
        needs_images: effort_from == C::Operate,
    };
    let only = |categories| Rating::Categories(categories);
    Some(match canonical_work(work) {
        "Architecture and broad changes" => (
            rule(only(&[C::Orchestrate, C::Design]), false, C::Orchestrate),
            None,
        ),
        "Bounded implementation" => (rule(only(&[C::Implement]), false, C::Implement), None),
        "Mechanical changes and search" => (
            rule(only(&[C::Implement, C::Scout]), true, C::Implement),
            None,
        ),
        "Adversarial review" => (rule(only(&[C::Review]), false, C::Review), None),
        "Computer use" => (rule(only(&[C::Operate]), false, C::Operate), None),
        "Conversation titles" => (rule(only(&[C::GitText]), true, C::GitText), None),
        FALLBACK_WORK => (rule(Rating::Overall, false, C::Orchestrate), None),
        work => {
            let agent = ProviderKind::parse(work.strip_prefix("fusion-")?)?;
            (rule(Rating::Overall, false, C::Orchestrate), Some(agent))
        }
    })
}
/// Whether a kind of work's rows are one model per agent, each for its own role, not an order.
fn by_role(work: &str) -> bool {
    same_work(work, "Adversarial review")
        || same_work(work, FALLBACK_WORK)
        || work.trim().starts_with("fusion-")
}
/// How an automatic row from the ratings starts its reason; a shipped row's reason never does.
const RATED_REASON: &str = "Rated ";
/// `work`'s picks from the ratings: each agent's best model it may start, best first.
fn rated_picks(
    work: &str,
    models: &[MergedModel],
    allowed: &dyn Fn(&MergedModel, &str) -> bool,
) -> Vec<brigadier_router::Pick> {
    let Some((rule, agent)) = pick_rule(work) else {
        return Vec::new();
    };
    brigadier_router::picks(models, rule, |model| {
        agent.is_none_or(|agent| agent == model.provider) && allowed(model, work)
    })
}
/// The automatic rows: per kind of work of the shipped table, in its order, the rows its
/// ratings give (one per agent, best first), then the shipped rows of agents with nothing rated
/// for it; where nothing is rated at all, the shipped rows. A shipped row `on` says can't run
/// (its agent switched off, its model made unavailable) is left out (#743), unless that leaves
/// its kind with no row at all.
fn automatic(
    shipped: Vec<GuidanceRow>,
    models: &[MergedModel],
    allowed: &dyn Fn(&MergedModel, &str) -> bool,
    on: &dyn Fn(&GuidanceRow) -> bool,
) -> Vec<GuidanceRow> {
    let mut out: Vec<GuidanceRow> = Vec::new();
    for row in &shipped {
        if out.iter().any(|done| same_work(&done.work, &row.work)) {
            continue;
        }
        let picks = rated_picks(&row.work, models, allowed);
        if picks.is_empty() {
            let kind: Vec<GuidanceRow> = shipped
                .iter()
                .filter(|other| same_work(&other.work, &row.work))
                .cloned()
                .collect();
            let running: Vec<GuidanceRow> = kind.iter().filter(|row| on(row)).cloned().collect();
            out.extend(if running.is_empty() { kind } else { running });
            continue;
        }
        let small = pick_rule(&row.work).is_some_and(|(rule, _)| rule.small);
        // An agent with no rated model for it keeps its shipped row, after the rated ones, when
        // that row can run: an agent switched off has no pick, and gets no row back either.
        let unrated: Vec<GuidanceRow> = shipped
            .iter()
            .filter(|other| {
                same_work(&other.work, &row.work)
                    && !picks.iter().any(|pick| pick.provider == other.agent)
                    && on(other)
            })
            .cloned()
            .collect();
        out.extend(picks.into_iter().map(|pick| GuidanceRow {
            work: row.work.clone(),
            agent: pick.provider,
            reason: format!(
                "{RATED_REASON}{:.1} of 10 for this work: {}'s best{} model by the ratings.",
                pick.score,
                pick.provider.label(),
                if small { " fast or mid-sized" } else { "" },
            ),
            model: pick.model,
            effort: pick.effort,
        }));
        out.extend(unrated);
    }
    out
}
/// The suggestions for the user's orders: for each kind of work they ordered, the best-rated
/// model when it rates at least [`SUGGEST_MARGIN`] above the first of their order, unless they
/// dismissed it at that rating. Reads `user` only: an order changes when the user takes one.
/// `listed` gives the listed model a row's model word starts (`default`: the CLI's default). A
/// kind of roles (review, Fallback, a fusion's consults) is one model per agent, not an order: each
/// agent's first row is compared with that agent's best model, and taking one replaces that row.
fn suggestions(
    user: &[GuidanceRow],
    registry: &brigadier_router::Registry,
    models: &[MergedModel],
    allowed: &dyn Fn(&MergedModel, &str) -> bool,
    listed: &dyn Fn(ProviderKind, &str) -> Option<String>,
    dismissed: &[crate::DismissedSuggestion],
) -> Vec<crate::RoutingSuggestion> {
    let mut found = Vec::new();
    for (index, first) in user.iter().enumerate() {
        let roles = by_role(&first.work);
        if user[..index]
            .iter()
            .any(|row| same_work(&row.work, &first.work) && (!roles || row.agent == first.agent))
        {
            continue;
        }
        let Some((rule, _)) = pick_rule(&first.work) else {
            continue;
        };
        let Some(current) = listed(first.agent, &first.model).and_then(|id| {
            models.iter().find(|model| {
                model.provider == first.agent && brigadier_router::is_model(model, registry, &id)
            })
        }) else {
            continue;
        };
        let Some(current_score) = brigadier_router::picks::score(current, rule.rating) else {
            continue;
        };
        let Some(best) = rated_picks(&first.work, models, allowed)
            .into_iter()
            .find(|pick| !roles || pick.provider == first.agent)
        else {
            continue;
        };
        let same = models.iter().any(|model| {
            model.provider == best.provider
                && model.id == best.model
                && model.provider == current.provider
                && model.rating_identity() == current.rating_identity()
        });
        if same || best.score + 1e-9 < current_score + crate::SUGGEST_MARGIN {
            continue;
        }
        if dismissed.iter().any(|gone| {
            same_work(&gone.work, &first.work)
                && gone.agent == best.provider
                && gone.model.eq_ignore_ascii_case(&best.model)
                && (gone.score - best.score).abs() < 0.05
        }) {
            continue;
        }
        found.push(crate::RoutingSuggestion {
            work: first.work.clone(),
            row: GuidanceRow {
                work: first.work.clone(),
                agent: best.provider,
                model: best.model.clone(),
                effort: best.effort.clone(),
                reason: format!(
                    "{RATED_REASON}{:.1} of 10 for this work; {} rates {current_score:.1}.",
                    best.score, first.model
                ),
            },
            score: best.score,
            current: first.model.clone(),
            current_score,
        });
    }
    found
}
/// The directive a rules-file line holds, with the line as the user wrote it; `None` for prose.
pub(super) fn directive(line: &str) -> Result<Option<(String, ModelRule)>> {
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
    // `forbid for Adversarial review: sonnet`: the rule holds for that kind of work only.
    let (key, work) = match key.split_once(" for ") {
        Some((key, work)) if !work.trim().is_empty() => {
            (key, Some(canonical_work(work).to_owned()))
        }
        _ => (key, None),
    };
    // `allow claude`, `effort-ceiling opus`: the directive, then what it is about.
    let key = key.trim();
    let (kind, target) = match key.split_once(' ') {
        Some((kind, target)) => (kind, Some(target.trim())),
        None => (key, None),
    };
    let parsed = match (kind, target) {
        ("forbid", None) => ModelRule::Forbid {
            names: names(),
            work,
        },
        ("allow", None) => ModelRule::Allow {
            agent: None,
            names: names(),
            work,
        },
        ("allow", Some(target)) if let Some(agent) = agent_named(target) => ModelRule::Allow {
            agent: Some(agent),
            names: names(),
            work,
        },
        ("effort-ceiling" | "effort-floor", target) => {
            // An agent's id, else one model name (no spaces, commas).
            let (agent, model) = match target {
                None => (None, None),
                Some(target) if let Some(agent) = agent_named(target) => (Some(agent), None),
                Some(target) if !target.is_empty() && !target.contains([' ', ',']) => {
                    (None, Some(target.to_owned()))
                }
                Some(_) => return Ok(None),
            };
            rank(value).ok_or_else(|| Error::Invalid(format!("invalid model rule: {rule}")))?;
            let effort = value.to_ascii_lowercase();
            if kind == "effort-floor" {
                ModelRule::EffortFloor {
                    effort,
                    agent,
                    model,
                    work,
                }
            } else {
                ModelRule::EffortCeiling {
                    effort,
                    agent,
                    model,
                    work,
                }
            }
        }
        ("advice", None) if work.is_none() && !value.is_empty() => ModelRule::Advice {
            text: value.to_owned(),
        },
        _ => return Ok(None),
    };
    Ok(Some((rule.into(), parsed)))
}
/// Whether an effort rule's agent or model is the choice's (none named: every choice).
fn effort_target(
    agent: Option<ProviderKind>,
    model: Option<&str>,
    provider: ProviderKind,
    models: &[&str],
) -> bool {
    agent.is_none_or(|agent| agent == provider)
        && model.is_none_or(|pattern| models.iter().any(|m| matches(pattern, m)))
}
pub(super) struct Policy {
    entries: Vec<(String, ModelRule)>,
    /// The lines that read as a rule but can't be one (an effort no level is), each
    /// `line N: text`. They hold nothing; the other rules still do (#762).
    invalid: Vec<String>,
}
impl Policy {
    /// The file's rules. A line that can't be a rule is left out and kept in [`Self::invalid`]:
    /// one typo must not empty the Fallback rows or refuse every move (#762).
    fn parse(text: &str) -> Self {
        let mut entries = Vec::new();
        let mut invalid = Vec::new();
        for (index, line) in text.lines().enumerate() {
            match directive(line) {
                Ok(found) => entries.extend(found),
                Err(_) => invalid.push(format!("line {}: {}", index + 1, line.trim())),
            }
        }
        Self { entries, invalid }
    }
    /// The rules, or the error the first line that can't be a rule makes: what an explicit
    /// start or model change answers, so the user hears of it.
    fn checked(self) -> Result<Self> {
        match self.invalid.first() {
            Some(line) => Err(Error::Invalid(format!(
                "invalid model rule in your rules file, {line}: fix or remove it in Settings › Routing"
            ))),
            None => Ok(self),
        }
    }
    /// Whether a rule holds for a start of `work`: a rule for one kind of work judges only a
    /// start for that work.
    fn holds(directive: &ModelRule, work: Option<&str>) -> bool {
        directive
            .work()
            .is_none_or(|scope| work.is_some_and(|work| same_work(scope, work)))
    }
    /// Rules for any of these scopes, evaluated together (a move must meet both its work and
    /// Fallback). Erase scopes only after selecting them, so effort bounds share one interval.
    fn for_works(mut self, works: &[Option<&str>]) -> Self {
        self.entries
            .retain(|(_, rule)| works.iter().any(|work| Self::holds(rule, *work)));
        for (_, rule) in &mut self.entries {
            match rule {
                ModelRule::Forbid { work, .. }
                | ModelRule::Allow { work, .. }
                | ModelRule::EffortCeiling { work, .. }
                | ModelRule::EffortFloor { work, .. } => *work = None,
                ModelRule::Advice { .. } => {}
            }
        }
        self
    }
    /// The first rule a choice breaks, quoted; effort rules refuse only when they can't be met
    /// (see [`Self::effort`]).
    fn refusal(
        &self,
        provider: ProviderKind,
        models: &[&str],
        effort: Option<&str>,
        work: Option<&str>,
    ) -> Option<String> {
        self.model_refusal(provider, models, work)
            .or_else(|| self.effort(provider, models, effort, &[], work).err())
    }
    /// The first Never or Only rule a choice's model breaks, quoted.
    fn model_refusal(
        &self,
        provider: ProviderKind,
        models: &[&str],
        work: Option<&str>,
    ) -> Option<String> {
        self.entries.iter().find_map(|(rule, directive)| {
            if !Self::holds(directive, work) {
                return None;
            }
            let violates = match directive {
                ModelRule::Forbid { names, .. } => {
                    models.iter().any(|m| names.iter().any(|p| matches(p, m)))
                }
                ModelRule::Allow { agent, names, .. } => {
                    agent.is_none_or(|p| p == provider)
                        && !models.is_empty()
                        && !models.iter().any(|m| names.iter().any(|p| matches(p, m)))
                }
                ModelRule::EffortCeiling { .. }
                | ModelRule::EffortFloor { .. }
                | ModelRule::Advice { .. } => false,
            };
            violates.then(|| format!("refused by your model rule: \"{rule}\""))
        })
    }
    /// The effort a choice runs at under the effort rules that hold for it (D-L38): one asked
    /// above the lowest cap comes down to it, one below the highest minimum (or none, with a
    /// minimum) goes up to it; with `listed` efforts, to the nearest of those within the rules.
    /// An effort within them stays as it is. `Err`, the refusal when a cap and a minimum can't
    /// both be met, or (`listed`) no effort of the model is within a rule.
    fn effort(
        &self,
        provider: ProviderKind,
        models: &[&str],
        effort: Option<&str>,
        listed: &[String],
        work: Option<&str>,
    ) -> std::result::Result<Option<String>, String> {
        // An effort rule that holds: its effort's rank, its effort, its line.
        type Limit<'a> = Option<(usize, &'a str, &'a str)>;
        let (mut floor, mut ceiling): (Limit, Limit) = (None, None);
        for (rule, directive) in &self.entries {
            if !Self::holds(directive, work) {
                continue;
            }
            match directive {
                ModelRule::EffortCeiling {
                    effort,
                    agent,
                    model,
                    ..
                } if effort_target(*agent, model.as_deref(), provider, models) => {
                    let r = rank(effort).unwrap_or(usize::MAX);
                    if ceiling.is_none_or(|(c, ..)| r < c) {
                        ceiling = Some((r, effort, rule));
                    }
                }
                ModelRule::EffortFloor {
                    effort,
                    agent,
                    model,
                    ..
                } if effort_target(*agent, model.as_deref(), provider, models) => {
                    let r = rank(effort).unwrap_or(0);
                    if floor.is_none_or(|(f, ..)| r > f) {
                        floor = Some((r, effort, rule));
                    }
                }
                _ => {}
            }
        }
        let refused = |rule: &str| format!("refused by your model rule: \"{rule}\"");
        if let (Some((f, _, floor)), Some((c, _, ceiling))) = (floor, ceiling)
            && f > c
        {
            return Err(format!(
                "refused by your model rules: \"{ceiling}\" and \"{floor}\" can't both be met"
            ));
        }
        let within =
            |r: usize| floor.is_none_or(|(f, ..)| f <= r) && ceiling.is_none_or(|(c, ..)| r <= c);
        let asked = effort.map(|e| (rank(e), e));
        let (limit, up) = match asked {
            Some((Some(r), e)) if within(r) => return Ok(Some(e.to_owned())),
            // No effort named, and no minimum: the CLI's own default stays.
            None if floor.is_none() => return Ok(None),
            Some((Some(r), _)) if ceiling.is_some_and(|(c, ..)| r > c) => (ceiling, false),
            Some((None, _)) if ceiling.is_some() => (ceiling, false),
            Some((None, e)) if floor.is_none() => return Ok(Some(e.to_owned())),
            _ => (floor, true),
        };
        let Some((_, effort, rule)) = limit else {
            return Ok(effort.map(str::to_owned));
        };
        let ranked: Vec<(usize, &String)> =
            listed.iter().filter_map(|e| Some((rank(e)?, e))).collect();
        if ranked.is_empty() {
            return Ok(Some(effort.to_owned()));
        }
        let fits = ranked.iter().filter(|(r, _)| within(*r));
        let nearest = if up {
            fits.min_by_key(|(r, _)| *r)
        } else {
            fits.max_by_key(|(r, _)| *r)
        };
        nearest
            .map(|(_, e)| Some((*e).clone()))
            .ok_or_else(|| refused(rule))
    }
    /// Whether a rule for every kind of work keeps some of `provider`'s models out.
    fn restricts_models(&self, provider: ProviderKind) -> bool {
        self.entries.iter().any(|(_, d)| match d {
            _ if d.work().is_some() => false,
            ModelRule::Forbid { names, .. } => !names.is_empty(),
            ModelRule::Allow { agent, .. } => agent.is_none_or(|p| p == provider),
            ModelRule::EffortCeiling { .. }
            | ModelRule::EffortFloor { .. }
            | ModelRule::Advice { .. } => false,
        })
    }
    /// The advice notes, in the file's order.
    fn advice(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter_map(|(_, d)| match d {
                ModelRule::Advice { text } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }
}
/// Whether two Work cells name the same kind of work, as `--work` finds a row.
pub(super) fn same_work(a: &str, b: &str) -> bool {
    canonical_work(a).eq_ignore_ascii_case(canonical_work(b))
}
/// A rule as the file writes it: `forbid: fable`, `allow claude: opus, sonnet`,
/// `effort-ceiling opus for Adversarial review: medium`, `advice: prefer Codex`.
pub(super) fn rule_text(rule: &ModelRule) -> String {
    let scope = rule
        .work()
        .map_or(String::new(), |work| format!(" for {}", work.trim()));
    let about = |agent: &Option<ProviderKind>, model: &Option<String>| match (agent, model) {
        (Some(agent), _) => format!(" {}", agent.id()),
        (None, Some(model)) => format!(" {}", model.trim()),
        (None, None) => String::new(),
    };
    match rule {
        ModelRule::Forbid { names, .. } => format!("forbid{scope}: {}", names.join(", ")),
        ModelRule::Allow { agent, names, .. } => format!(
            "allow{}{scope}: {}",
            agent.map_or(String::new(), |p| format!(" {}", p.id())),
            names.join(", ")
        ),
        ModelRule::EffortCeiling {
            effort,
            agent,
            model,
            ..
        } => format!("effort-ceiling{}{scope}: {effort}", about(agent, model)),
        ModelRule::EffortFloor {
            effort,
            agent,
            model,
            ..
        } => format!("effort-floor{}{scope}: {effort}", about(agent, model)),
        ModelRule::Advice { text } => format!("advice: {}", text.trim()),
    }
}
const TABLE_HEAD: &str =
    "| Work | Agent | Model | Effort | Reason |\n| --- | --- | --- | --- | --- |";
/// Checks what Settings › Routing sends before it reaches the file: every cell fits the file's
/// one-line table, names and a work fit a rule, efforts are levels, and no work names the same
/// model twice in its order.
pub(super) fn check_rules(rows: &[GuidanceRow], rules: &[ModelRule]) -> Result<()> {
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
        if rows[..i].iter().any(|other| {
            other.work == row.work
                && other.agent == row.agent
                && other.model.eq_ignore_ascii_case(&row.model)
        }) {
            return bad(&format!("\"{}\" names {} twice", row.work, row.model));
        }
    }
    for rule in rules {
        if rule.work().is_some_and(|work| {
            work.trim().is_empty() || work.contains([':', '\n', '\r']) || work.contains(" for ")
        }) {
            return bad("a rule's work cannot be empty or hold a colon, a line break or \" for \"");
        }
        match rule {
            ModelRule::Forbid { names, .. } | ModelRule::Allow { names, .. } => {
                if names.is_empty()
                    || names
                        .iter()
                        .any(|n| n.trim().is_empty() || n.contains([',', ':', '\n', '\r']))
                {
                    return bad("a rule names models without commas, colons or line breaks");
                }
            }
            ModelRule::EffortCeiling {
                effort,
                agent,
                model,
                ..
            }
            | ModelRule::EffortFloor {
                effort,
                agent,
                model,
                ..
            } => {
                if rank(effort).is_none() {
                    return bad("unknown effort level");
                }
                if agent.is_some() && model.is_some() {
                    return bad("an effort rule names an agent or a model, not both");
                }
                if model.as_deref().is_some_and(|m| {
                    m.trim().is_empty()
                        || m.contains([' ', ',', ':', '\n', '\r'])
                        || agent_named(m.trim()).is_some()
                }) {
                    return bad("an effort rule names one model, without spaces, commas or colons");
                }
            }
            ModelRule::Advice { text } => {
                if text.trim().is_empty() || text.contains(['\n', '\r']) {
                    return bad("advice is one line of words");
                }
            }
        }
    }
    Ok(())
}
/// The rules file with its rows and rules replaced, in the same text format: prose and comments
/// stay where they are, including unrelated tables and rows this version cannot read. The rules
/// take the place of the first rule line and the table the first readable row (else at the end).
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
            r.agent.id(),
            r.model.trim(),
            r.effort.as_deref().unwrap_or("").trim(),
            r.reason.trim()
        )
    }));
    let (mut rules_done, mut table_done) = (false, false);
    let mut out: Vec<String> = Vec::new();
    let readable = routing_rows(text);
    let mut lines = text.lines().enumerate().peekable();
    while let Some((index, line)) = lines.next() {
        // Only consume our header and its separator, never an unrelated table's header.
        if routing_header(line) {
            if lines.peek().is_some_and(|(_, next)| table_separator(next)) {
                lines.next();
            }
            // Keep the header when unreadable rows remain underneath it.
            let unreadable = lines
                .clone()
                .take_while(|(_, line)| line.trim_start().starts_with('|'))
                .any(|(index, _)| !readable.iter().any(|(at, _)| *at == index));
            if !rows.is_empty() || unreadable {
                out.push(line.to_owned());
                out.push("| --- | --- | --- | --- | --- |".to_owned());
                if !table_done {
                    out.extend(table.iter().skip(1).cloned());
                    table_done = true;
                }
            }
        } else if readable.iter().any(|(at, _)| *at == index) {
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
/// A revision covers the whole file, including advice/prose not represented in the page's rows.
fn rules_revision(text: &str) -> String {
    use base64::Engine;
    use sha2::{Digest, Sha256};
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(text.as_bytes()))
}
fn save_rules(
    path: &std::path::Path,
    revision: &str,
    rows: &[GuidanceRow],
    rules: &[ModelRule],
) -> Result<()> {
    // Serialize the revision check and rename for all saves handled by this daemon.
    static SAVING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _saving = SAVING.lock().unwrap_or_else(|e| e.into_inner());
    check_rules(rows, rules)?;
    let current = editable(path, DEFAULT_RULES)?;
    if rules_revision(&current) != revision {
        return Err(Error::Invalid("Routing changed since this page was read. Reopen Routing and try again; your changes were not saved.".into()));
    }
    let text = rewrite(&current, rows, rules);
    let io = |e: std::io::Error| Error::Invalid(format!("{}: {e}", path.display()));
    let staged = path.with_extension("md.saving");
    std::fs::write(&staged, text).map_err(io)?;
    std::fs::rename(&staged, path).map_err(io)?;
    Ok(())
}
impl SessionManager {
    pub(super) fn models_rules_path(&self) -> PathBuf {
        if cfg!(test) {
            return self.data_dir.join("models.md");
        }
        rules_file(
            std::env::var_os("BRIGADIER_MODELS_FILE"),
            brigadier_sandbox::default_data_dir().ok().as_deref(),
            &self.data_dir,
            std::env::home_dir(),
        )
    }
    /// Settings › Routing's read of the rules file, as it is on disk now (S32), beside the
    /// automatic rows the ratings give and the suggestions for the user's own orders.
    pub fn model_rules(&self) -> Result<ModelRules> {
        let path = self.models_rules_path();
        let text = editable(&path, DEFAULT_RULES)?;
        let (shipped, suggestions) = self.automatic_view(&text)?;
        Ok(ModelRules {
            path: path.display().to_string(),
            revision: rules_revision(&text),
            shipped,
            rows: rows(&text),
            rules: Policy::parse(&text)
                .entries
                .into_iter()
                .map(|(_, rule)| rule)
                .collect(),
            suggestions,
            research: self.runtime.registry().rankings_refresh(),
        })
    }
    /// The suggestions for the user's orders in the rules file now.
    pub(super) fn routing_suggestions(&self) -> Result<Vec<crate::RoutingSuggestion>> {
        let text = editable(&self.models_rules_path(), DEFAULT_RULES)?;
        Ok(self.automatic_view(&text)?.1)
    }
    /// The automatic rows and the suggestions for the rules file's `text`. A model counts when
    /// the user left it on and no rule for that work keeps it out.
    fn automatic_view(
        &self,
        text: &str,
    ) -> Result<(Vec<GuidanceRow>, Vec<crate::RoutingSuggestion>)> {
        let (registry, models) = self.rated_models();
        let settings = self.core.settings();
        let policy = Policy::parse(text);
        let allowed = |model: &MergedModel, work: &str| {
            let names: Vec<&str> = std::iter::once(model.id.as_str())
                .chain(model.resolved.as_deref())
                .collect();
            crate::routing::availability::model_available(&settings, model.provider, &model.id)
                && policy
                    .refusal(model.provider, &names, None, Some(work))
                    .is_none()
        };
        let listed = |provider: ProviderKind, word: &str| {
            if word.trim().is_empty() || word.eq_ignore_ascii_case("default") {
                let catalog = self.runtime.overview(provider).and_then(|o| o.models)?;
                return catalog
                    .models
                    .iter()
                    .find(|model| model.is_default)
                    .map(|model| model.id.clone());
            }
            Some(self.listed_model(provider, word))
        };
        let shipped = automatic(rows(GUIDANCE), &models, &allowed, &|row| {
            self.row_available(row.agent, &row.model)
        });
        let found = suggestions(
            &rows(text),
            &registry,
            &models,
            &allowed,
            &listed,
            &settings.dismissed_suggestions,
        );
        Ok((shipped, found))
    }
    /// Settings › Routing's save: the file's rows and rules become these, in the file's own text
    /// format, its prose kept (S32).
    pub fn set_model_rules(
        &self,
        revision: &str,
        rows: &[GuidanceRow],
        rules: &[ModelRule],
    ) -> Result<ModelRules> {
        save_rules(&self.models_rules_path(), revision, rows, rules)?;
        self.model_rules()
    }
    /// `brigadier start --work`'s rows: the user's order of that work, else the shipped rows,
    /// first to last (S32).
    /// Every kind of work the routing names, in its order, once each.
    pub(super) fn work_kinds(&self) -> Result<Vec<String>> {
        let user = editable(&self.models_rules_path(), DEFAULT_RULES)?;
        let mut kinds: Vec<String> = Vec::new();
        for row in guidance(self.automatic_view(&user)?.0, &user) {
            if !kinds.iter().any(|kind| same_work(kind, &row.work)) {
                kinds.push(row.work);
            }
        }
        Ok(kinds)
    }

    pub(super) fn work_rows(&self, work: &str) -> Result<Vec<Guidance>> {
        let user = editable(&self.models_rules_path(), DEFAULT_RULES)?;
        Ok(guidance(self.automatic_view(&user)?.0, &user)
            .into_iter()
            .filter(|row| same_work(&row.work, work))
            .collect())
    }
    /// Conversation titles use normal Work rows, scoped rules, model switches and account
    /// routing. Shipped rows name small families; never widen them to a default/strong model.
    /// The registry's tier and short-text rating are qualitative cost/suitability advice, not
    /// an account price. A user's rows replace these candidates with their explicit order.
    pub(super) async fn title_route(
        &self,
    ) -> Result<Option<(crate::model::ModelChoice, crate::accounts::AccountRef)>> {
        use brigadier_router::{Heat, QualityTier, TaskCategory};
        const WORK: &str = "Conversation titles";
        let rows = self.work_rows(WORK)?;
        let inputs = self.routing_inputs(now_ms()).await;
        let settings = self.core.settings();
        let mut candidates = Vec::new();
        for (position, row) in rows.into_iter().enumerate() {
            let provider = row.agent;
            // A default model can silently become a frontier model: there is no implicit
            // default fallback for this chore, even when a light model is unavailable.
            if row.model.trim().is_empty()
                || row.model.eq_ignore_ascii_case("default")
                || !self.provider_ready(provider)
            {
                continue;
            }
            // Automatic rows (from the ratings, or the shipped table) weigh quota and tier.
            let shipped = row.source != "user";
            let selected = self.listed_model(provider, &row.model);
            let models = inputs.models.iter().filter(|rated| {
                rated.provider == provider
                    && if shipped {
                        brigadier_router::is_model(rated, &inputs.registry, &row.model)
                            || rated
                                .family
                                .as_deref()
                                .is_some_and(|f| f.eq_ignore_ascii_case(&row.model))
                            || rated
                                .id
                                .split(['-', '_'])
                                .any(|word| word.eq_ignore_ascii_case(&row.model))
                    } else {
                        brigadier_router::is_model(rated, &inputs.registry, &selected)
                    }
            });
            for rated in models {
                let model = &rated.id;
                if rated.excluded
                    || rated
                        .only
                        .as_ref()
                        .is_some_and(|only| !only.contains_key(&TaskCategory::GitText))
                    || (shipped
                        && !matches!(rated.tier, QualityTier::Light | QualityTier::Standard))
                    || !crate::routing::availability::model_available(&settings, provider, model)
                    || self
                        .model_refusal(provider, Some(model), row.effort.as_deref(), Some(WORK))?
                        .is_some()
                {
                    continue;
                }
                let account = self.runtime.launch_account(provider, Some(model));
                let choice = self.ruled_choice(
                    crate::model::ModelChoice {
                        provider,
                        model: Some(model.clone()),
                        effort: row.effort.clone(),
                        fast: None,
                        account: account.account.clone(),
                    },
                    Some(WORK),
                );
                if !self.account_frees_choice(&choice, &account).await {
                    continue;
                }
                let quota = self.runtime.account_usage(&account, now_ms());
                let heat = quota
                    .as_ref()
                    .into_iter()
                    .flat_map(|q| &q.windows)
                    .filter(|w| {
                        brigadier_router::window_applies(&w.window, rated, &inputs.registry)
                            && w.window.resets_at_ms.is_none_or(|reset| reset > now_ms())
                    })
                    .map(|w| w.heat)
                    .max()
                    .unwrap_or(Heat::Cool);
                let suitability = rated
                    .strengths
                    .get(&TaskCategory::GitText)
                    .copied()
                    .unwrap_or(5.0);
                if suitability <= 0.0 {
                    continue;
                }
                candidates.push((
                    if shipped { heat } else { Heat::Cool },
                    if shipped {
                        rated.tier
                    } else {
                        QualityTier::Light
                    },
                    if shipped {
                        -(suitability * 100.0) as i64
                    } else {
                        0
                    },
                    position,
                    choice,
                    account,
                ));
            }
        }
        candidates.sort_by_key(|(heat, tier, fit, position, _, _)| (*heat, *tier, *fit, *position));
        Ok(candidates
            .into_iter()
            .next()
            .map(|(_, _, _, _, choice, account)| (choice, account)))
    }
    /// The `Fallback` rows of the user's routing, in its order (else the shipped ones), as the
    /// choices a session moves to another agent with (S38, D25, D34). A `default` model is the
    /// CLI's own default, as the row says.
    pub(super) fn fallback_rows(&self) -> Vec<crate::model::ModelChoice> {
        self.work_rows(FALLBACK_WORK)
            .unwrap_or_default()
            .into_iter()
            .map(|row| {
                let provider = row.agent;
                let choice = crate::model::ModelChoice {
                    provider,
                    model: (!row.model.is_empty() && !row.model.eq_ignore_ascii_case("default"))
                        .then(|| self.listed_model(provider, &row.model)),
                    effort: row.effort,
                    fast: None,
                    account: None,
                };
                self.ruled_choice(choice, Some(FALLBACK_WORK))
            })
            .collect()
    }
    /// A row's model as its CLI takes it: a listed id as it is; a family word (`sol`, what the
    /// page's "Newest … sol" saves) the newest listed model it names, so it follows releases;
    /// anything else as written.
    pub(super) fn listed_model(&self, provider: ProviderKind, model: &str) -> String {
        let Some(catalog) = self.runtime.overview(provider).and_then(|o| o.models) else {
            return model.to_owned();
        };
        let listed = |id: &str| id.eq_ignore_ascii_case(model);
        if catalog
            .models
            .iter()
            .any(|m| listed(&m.id) || m.resolved.as_deref().is_some_and(listed))
        {
            return model.to_owned();
        }
        let word = model.to_ascii_lowercase();
        let version = |id: &str| -> Vec<u32> {
            id.split(|c: char| !c.is_ascii_digit())
                .filter_map(|part| part.parse().ok())
                .filter(|number| *number < 1000)
                .collect()
        };
        catalog
            .models
            .iter()
            .filter(|m| !m.legacy)
            .filter(|m| {
                m.id.split(['-', '_', ' '])
                    .any(|part| part.eq_ignore_ascii_case(&word))
            })
            .max_by_key(|m| version(&m.id))
            .map_or_else(|| model.to_owned(), |m| m.id.clone())
    }
    /// What a model and effort of `provider` run as (D77: no false off-routing line).
    pub(super) fn runs_as(
        &self,
        provider: ProviderKind,
        model: Option<&str>,
        effort: Option<&str>,
    ) -> (Option<String>, Option<String>) {
        let catalog = self.runtime.overview(provider).and_then(|o| o.models);
        runs_as(catalog.as_ref(), model, effort)
    }
    /// The user's rules, without any line that can't be a rule (named once in the log): what
    /// every start and move is judged by, so one typo leaves the other rules and Fallback on.
    /// Whether a routing row of `agent` naming `model` can start as Settings › Providers stands:
    /// the agent switched on, and the model it runs as (a family word's newest listed model,
    /// `default` the CLI's default) not made unavailable. Settings › Routing judges it the same.
    pub(super) fn row_available(&self, agent: ProviderKind, model: &str) -> bool {
        use crate::routing::availability::{model_available, provider_on};
        let settings = self.core.settings();
        if !provider_on(&settings, agent) {
            return false;
        }
        let catalog = self.runtime.overview(agent).and_then(|o| o.models);
        // The catalog's own id: what Providers records a model made unavailable as.
        let id = if model.trim().is_empty() || model.eq_ignore_ascii_case("default") {
            match listed(catalog.as_ref(), None) {
                Some(default) => default.id.clone(),
                None => return true,
            }
        } else {
            let name = self.listed_model(agent, model);
            listed(catalog.as_ref(), Some(&name)).map_or(name, |entry| entry.id.clone())
        };
        model_available(&settings, agent, &id)
    }
    pub(super) fn model_policy(&self) -> Result<Policy> {
        let policy = Policy::parse(&editable(&self.models_rules_path(), DEFAULT_RULES)?);
        note_invalid(&policy.invalid);
        Ok(policy)
    }
    /// Refuses an explicit action (a start, a switch, a model change, a move the user asks for)
    /// while a line of the rules file can't be a rule, naming it (#762).
    pub(super) fn check_rules_file(&self) -> Result<()> {
        Policy::parse(&editable(&self.models_rules_path(), DEFAULT_RULES)?)
            .checked()
            .map(|_| ())
    }
    /// The rule that refuses a choice, quoted; `work`, the kind of work a start is for.
    pub(super) fn model_refusal(
        &self,
        provider: ProviderKind,
        model: Option<&str>,
        effort: Option<&str>,
        work: Option<&str>,
    ) -> Result<Option<String>> {
        let catalog = self.runtime.overview(provider).and_then(|o| o.models);
        let selected = listed(catalog.as_ref(), model);
        let names: Vec<_> = model
            .into_iter()
            .chain(selected.map(|m| m.id.as_str()))
            .chain(selected.and_then(|m| m.resolved.as_deref()))
            .collect();
        let effort = effort.or(selected.and_then(|m| m.default_effort.as_deref()));
        let listed = selected.map(|m| m.efforts.as_slice()).unwrap_or_default();
        let policy = self.model_policy()?;
        Ok(policy
            .model_refusal(provider, &names, work)
            .or_else(|| policy.effort(provider, &names, effort, listed, work).err()))
    }
    /// The effort a choice runs at under the user's effort rules (D-L38): `effort` as asked
    /// when the rules allow it (`None`, the model's default, stays `None`), else moved to the
    /// cap or the minimum. A choice the rules refuse keeps its effort; [`Self::model_refusal`]
    /// says why.
    pub(super) fn ruled_effort(
        &self,
        provider: ProviderKind,
        model: Option<&str>,
        effort: Option<&str>,
        work: Option<&str>,
    ) -> Result<Option<String>> {
        let catalog = self.runtime.overview(provider).and_then(|o| o.models);
        let selected = listed(catalog.as_ref(), model);
        let names: Vec<_> = model
            .into_iter()
            .chain(selected.map(|m| m.id.as_str()))
            .chain(selected.and_then(|m| m.resolved.as_deref()))
            .collect();
        let judged = effort.or(selected.and_then(|m| m.default_effort.as_deref()));
        let listed = selected.map(|m| m.efforts.as_slice()).unwrap_or_default();
        Ok(
            match self
                .model_policy()?
                .effort(provider, &names, judged, listed, work)
            {
                Ok(ruled) if ruled.as_deref() == judged => effort.map(str::to_owned),
                Ok(ruled) => ruled,
                Err(_) => effort.map(str::to_owned),
            },
        )
    }
    /// `choice` with its effort as the user's effort rules move it, for `work`.
    pub(super) fn ruled_choice(
        &self,
        mut choice: crate::model::ModelChoice,
        work: Option<&str>,
    ) -> crate::model::ModelChoice {
        if let Ok(effort) = self.ruled_effort(
            choice.provider,
            choice.model.as_deref(),
            choice.effort.as_deref(),
            work,
        ) {
            choice.effort = effort;
        }
        choice
    }
    /// A quota retry on its own agent follows its work; a move also follows Fallback rules.
    pub(super) fn ruled_quota_choice(
        &self,
        mut choice: crate::model::ModelChoice,
        work: Option<&str>,
        moving: bool,
    ) -> Result<crate::model::ModelChoice> {
        let mut works = vec![work];
        if moving {
            works.push(Some(FALLBACK_WORK));
        }
        let policy = self.model_policy()?.for_works(&works);
        let catalog = self
            .runtime
            .overview(choice.provider)
            .and_then(|o| o.models);
        let selected = listed(catalog.as_ref(), choice.model.as_deref());
        let names: Vec<_> = std::iter::once(choice.model.as_deref().unwrap_or("default"))
            .chain(selected.map(|m| m.id.as_str()))
            .chain(selected.and_then(|m| m.resolved.as_deref()))
            .collect();
        if let Some(reason) = policy.model_refusal(choice.provider, &names, None) {
            return Err(Error::Invalid(reason));
        }
        let judged = choice
            .effort
            .as_deref()
            .or(selected.and_then(|m| m.default_effort.as_deref()));
        let efforts = selected.map(|m| m.efforts.as_slice()).unwrap_or_default();
        let effort = policy
            .effort(choice.provider, &names, judged, efforts, None)
            .map_err(Error::Invalid)?;
        if effort.as_deref() != judged {
            choice.effort = effort;
        }
        Ok(choice)
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
            let denied = policy.refusal(provider, &names, None, None).is_some();
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
        if !allowed.ids.is_empty() && allowed.outside.is_empty() && !policy.entries.iter().any(|(_, d)| matches!(d, ModelRule::Allow { agent, work: None, .. } if agent.is_none_or(|p| p == provider))) {
            return Ok(None);
        }
        Ok(Some(allowed))
    }
    pub(super) async fn cli_models(&self, caller: &super::cli::CliCaller) -> Result<ModelsOutput> {
        let now = now_ms();
        let rules_path = self.models_rules_path();
        let user = editable(&rules_path, DEFAULT_RULES)?;
        let policy = Policy::parse(&user);
        let shipped = self.automatic_view(&user)?.0;
        let settings = self.core.settings();
        let mut models = Vec::new();
        for provider in brigadier_providers::adapters::kinds() {
            if let Some(catalog) = self.runtime.overview(provider).and_then(|o| o.models) {
                let age_s = Some((now - catalog.fetched_at_ms).max(0) / 1000);
                for model in catalog.models {
                    models.push(Model {
                        agent: provider,
                        // Its agent on and the model not made unavailable (#743).
                        available: crate::routing::availability::model_available(
                            &settings, provider, &model.id,
                        ),
                        model: model.id,
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
                    now,
                );
                let why = a.reason.as_ref().map_or_else(
                    || "not ordered yet".into(),
                    |reason| order_words(reason, now),
                );
                Account {
                    place: a.place,
                    why,
                    id: a.account.account.clone().unwrap_or_else(|| "own".into()),
                    name: a.name,
                    agent: a.account.provider,
                    current: a.in_use,
                    you: current.as_ref().is_some_and(|cli| cli.account == a.account),
                    logged_in: a.status.is_some_and(|s| s.logged_in),
                    age_s: a.quota_at_ms.map(|at| (now - at).max(0) / 1000),
                    fit,
                    usage,
                }
            })
            .collect();
        let agents = brigadier_providers::adapters::kinds()
            .map(|provider| AgentAccounts {
                agent: provider,
                switch_accounts: settings.switches_accounts(provider),
            })
            .collect();
        Ok(ModelsOutput {
            workers_at_once: settings.workers_at_once(),
            models,
            agents,
            switch_providers: settings.switch_providers,
            accounts,
            guidance: guidance(shipped, &user),
            advice: policy.advice(),
            rules: policy
                .entries
                .into_iter()
                .filter(|(_, directive)| !matches!(directive, ModelRule::Advice { .. }))
                .map(|(rule, _)| Rule { rule })
                .collect(),
            rules_path: rules_path.display().to_string(),
            invalid_rules: policy.invalid,
        })
    }
}
/// Logs the rules file's lines that can't be a rule once per change of them, not at every check.
fn note_invalid(invalid: &[String]) {
    static NOTED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    let mut noted = NOTED.lock().unwrap_or_else(|e| e.into_inner());
    if noted.as_slice() != invalid {
        if !invalid.is_empty() {
            tracing::warn!(lines = ?invalid, "rules file lines that can't be a rule are ignored");
        }
        *noted = invalid.to_vec();
    }
}
/// How long from `now` until `at`, in a few letters: `40m`, `5h`, `3d`.
fn within(at: i64, now: i64) -> String {
    let minutes = ((at - now).max(0) + 59_999) / 60_000;
    match minutes {
        m if m < 60 => format!("{m}m"),
        m if m < 48 * 60 => format!("{}h", m / 60),
        m => format!("{}d", m / (24 * 60)),
    }
}
/// Why an account stands where it does in its agent's order, in words.
fn order_words(reason: &crate::accounts::OrderReason, now: i64) -> String {
    use crate::accounts::OrderReason;
    let within = |at: i64| within(at, now);
    match reason {
        OrderReason::Picked => "picked with Use".into(),
        OrderReason::ResetsSooner { resets_at_ms } => {
            format!("weekly resets first (in {})", within(*resets_at_ms))
        }
        OrderReason::NearerLimit {
            window,
            used_percent,
        } => format!("nearest its limit ({window} {used_percent:.0}% used)"),
        OrderReason::Added => "added first".into(),
        OrderReason::Last => "last".into(),
        OrderReason::AtLimit { ready_at_ms } => match ready_at_ms {
            Some(at) => format!("at its limit, back in {}", within(*at)),
            None => "at its limit".into(),
        },
        OrderReason::SignedOut => "not signed in".into(),
    }
}
/// An account's fit in a word: `ok`, `tight`, `out`, `unknown`, or out until a reset, given
/// with its date and how far away it is (a weekly window can be days out: #764).
fn fit(windows: &[Usage], limited: Option<Option<i64>>, now: i64) -> String {
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
        let when = jiff::Timestamp::from_millisecond(at).map_or_else(
            |_| format!("{at} ms"),
            |at| at.strftime("%Y-%m-%d %H:%M UTC").to_string(),
        );
        return format!("out until {when} (in {})", within(at, now));
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
/// The catalog's entry for `model`, matched by id or resolved id; none is its default model.
fn listed<'a>(
    catalog: Option<&'a brigadier_providers::ModelCatalog>,
    model: Option<&str>,
) -> Option<&'a brigadier_providers::ModelInfo> {
    catalog?.models.iter().find(|m| {
        model.map_or(m.is_default, |id| {
            m.id.eq_ignore_ascii_case(id)
                || m.resolved
                    .as_deref()
                    .is_some_and(|r| r.eq_ignore_ascii_case(id))
        })
    })
}

/// `opus` and the `claude-opus-5-5` it resolves to are one model; none is the CLI's default
/// model, and no effort that model's default effort; a model the catalog doesn't list is its
/// own id. Both lowercased.
fn runs_as(
    catalog: Option<&brigadier_providers::ModelCatalog>,
    model: Option<&str>,
    effort: Option<&str>,
) -> (Option<String>, Option<String>) {
    let entry = listed(catalog, model);
    let model = entry
        .map(|m| m.resolved.as_deref().unwrap_or(&m.id))
        .or(model);
    let effort = effort.or(entry.and_then(|m| m.default_effort.as_deref()));
    (
        model.map(str::to_ascii_lowercase),
        effort.map(str::to_ascii_lowercase),
    )
}

/// The rules file: the one `BRIGADIER_MODELS_FILE` names, else the user's in their home when
/// `data_dir` is the `installed` app's, else that data dir's own: a dev build or a custom data
/// dir, however it was given, never the user's (D31).
fn rules_file(
    named: Option<std::ffi::OsString>,
    installed: Option<&std::path::Path>,
    data_dir: &std::path::Path,
    home: Option<PathBuf>,
) -> PathBuf {
    match named {
        Some(file) => PathBuf::from(file),
        None if installed == Some(data_dir) => home
            .unwrap_or_else(|| data_dir.to_owned())
            .join(".brigadier/models.md"),
        None => data_dir.join("models.md"),
    }
}

#[cfg(test)]
mod tests {
    /// A dev build or custom data dir keeps its own routing whether it came from
    /// `BRIGADIER_DATA_DIR`, `--data-dir` or `--brigadier-data-dir`: setting a row there never
    /// writes the user's (demo8 wrote it); the installed app keeps the user's own.
    #[test]
    fn only_the_installed_data_dir_uses_the_rules_in_home() {
        let data = std::path::Path::new("/tmp/scratch-data");
        let installed = std::path::Path::new("/Users/me/Library/Application Support/Brigadier");
        let home = Some(PathBuf::from("/Users/me"));
        assert_eq!(
            super::rules_file(None, Some(installed), data, home.clone()),
            data.join("models.md")
        );
        assert_eq!(
            super::rules_file(None, None, data, home.clone()),
            data.join("models.md")
        );
        assert_eq!(
            super::rules_file(None, Some(installed), installed, home.clone()),
            PathBuf::from("/Users/me/.brigadier/models.md")
        );
        assert_eq!(
            super::rules_file(Some("/x/m.md".into()), Some(installed), installed, home),
            PathBuf::from("/x/m.md")
        );
    }

    use super::*;
    #[test]
    fn an_alias_its_id_the_default_and_its_effort_are_one_choice() {
        let model = |id: &str, resolved: Option<&str>, is_default: bool, effort: Option<&str>| {
            brigadier_providers::ModelInfo {
                id: id.into(),
                display_name: id.into(),
                description: String::new(),
                resolved: resolved.map(Into::into),
                efforts: vec![],
                default_effort: effort.map(Into::into),
                is_default,
                input_modalities: vec![],
                fast: None,
                legacy: false,
            }
        };
        let catalog = brigadier_providers::ModelCatalog {
            provider: ProviderKind::Claude,
            models: vec![
                model("opus", Some("claude-opus-5-5"), true, Some("high")),
                model("claude-opus-5-5", None, false, Some("high")),
                model("sonnet", Some("claude-sonnet-5-5"), false, None),
            ],
            cli_version: None,
            fetched_at_ms: 0,
        };
        let same = |a, b| runs_as(Some(&catalog), a, None) == runs_as(Some(&catalog), b, None);
        assert!(same(Some("opus"), Some("claude-opus-5-5")));
        assert!(same(Some("Claude-Opus-5-5"), Some("opus")));
        assert!(same(None, Some("claude-opus-5-5")));
        assert!(!same(Some("sonnet"), Some("opus")));
        assert!(!same(None, Some("sonnet")));
        assert!(same(Some("unlisted"), Some("UNLISTED")));
        assert!(!same(Some("unlisted"), None));
        assert_eq!(
            runs_as(None, Some("opus"), None),
            runs_as(None, Some("opus"), None)
        );
        assert_ne!(
            runs_as(None, Some("opus"), None),
            runs_as(None, Some("claude-opus-5-5"), None)
        );
        // No effort is the model's default effort; the default model runs as its resolved id.
        let opus = runs_as(Some(&catalog), Some("opus"), Some("HIGH"));
        assert_eq!(opus, runs_as(Some(&catalog), None, None));
        assert_eq!(opus.0.as_deref(), Some("claude-opus-5-5"));
        assert_ne!(opus, runs_as(Some(&catalog), None, Some("low")));
        assert_ne!(
            runs_as(Some(&catalog), Some("sonnet"), None),
            runs_as(Some(&catalog), Some("sonnet"), Some("high"))
        );
    }
    /// A small registry: one Claude model, two Codex models (one kept for computer use only).
    fn small_catalog() -> (brigadier_router::Registry, Vec<MergedModel>) {
        let entry = |key: &str, cli: &str, tier: &str, strengths: serde_json::Value| {
            serde_json::json!({
                "key": key, "vendor": if cli == "claude" { "anthropic" } else { "openai" },
                "cli": cli, "match": {"ids": [key]}, "tier": tier,
                "efforts": ["low", "medium", "high"], "strengths": strengths,
                "modalities": {"input": ["text", "image"]},
                "defaultEffort": {"implement": "medium", "operate": "high", "review": "high"},
                "released": "2026-01-01", "sources": ["https://example.com"]
            })
        };
        let mut eyes = entry(
            "eyes",
            "codex",
            "light",
            serde_json::json!({"operate": 9.6}),
        );
        eyes["only"] = serde_json::json!({"operate": "strong"});
        let document = serde_json::json!({
            "schemaVersion": 1, "revision": 999_999, "updated": "2026-11-01",
            "models": [
                entry("big", "claude", "frontier",
                    serde_json::json!({"implement": 8, "review": 9, "operate": 9})),
                entry("sol", "codex", "strong",
                    serde_json::json!({"implement": 9, "review": 8, "operate": 7})),
                eyes,
            ]
        });
        let registry =
            brigadier_router::Registry::parse(&serde_json::to_vec(&document).unwrap()).unwrap();
        let info = |id: &str| -> brigadier_providers::ModelInfo {
            serde_json::from_value(serde_json::json!({
                "id": id, "displayName": id, "description": "",
                "efforts": ["low", "medium", "high"], "isDefault": false,
                "inputModalities": ["text", "image"], "legacy": false
            }))
            .unwrap()
        };
        let models = brigadier_router::merge(
            &registry,
            &[
                (ProviderKind::Claude, &[info("big")]),
                (ProviderKind::Codex, &[info("sol"), info("eyes")]),
            ],
            &[],
            &[],
        );
        (registry, models)
    }

    /// #743: an agent switched off has no rated pick, and its shipped row doesn't come back in
    /// its place; a kind only that agent's rows name keeps them, so it still exists.
    #[test]
    fn automatic_leaves_out_a_switched_off_agent() {
        let (_, models) = small_catalog();
        let rows = automatic(
            rows(GUIDANCE),
            &models,
            &|model, _| model.provider != ProviderKind::Claude,
            &|row| row.agent != ProviderKind::Claude,
        );
        let shipped = super::rows(GUIDANCE);
        let claude_only = |work: &str| {
            shipped
                .iter()
                .filter(|row| same_work(&row.work, work))
                .all(|row| row.agent == ProviderKind::Claude)
        };
        for row in &rows {
            assert!(
                row.agent != ProviderKind::Claude || claude_only(&row.work),
                "{row:?}"
            );
        }
        assert!(rows.iter().any(|row| same_work(&row.work, "fusion-claude")));
        assert!(
            rows.iter()
                .any(|row| same_work(&row.work, "Bounded implementation"))
        );
    }
    /// The automatic rows come from the ratings, one per agent in rating order; a kind with
    /// nothing rated keeps the shipped table's rows.
    #[test]
    fn automatic_rows_follow_the_ratings_and_fall_back_to_the_shipped_table() {
        let (_, models) = small_catalog();
        let rows = automatic(rows(GUIDANCE), &models, &|_, _| true, &|_| true);
        let of = |work: &str| -> Vec<(ProviderKind, String, Option<String>)> {
            rows.iter()
                .filter(|row| row.work == work)
                .map(|row| (row.agent, row.model.clone(), row.effort.clone()))
                .collect()
        };
        assert_eq!(
            of("Bounded implementation"),
            [
                (ProviderKind::Codex, "sol".into(), Some("medium".into())),
                (ProviderKind::Claude, "big".into(), Some("medium".into())),
            ]
        );
        assert_eq!(
            of("Adversarial review"),
            [
                (ProviderKind::Claude, "big".into(), Some("high".into())),
                (ProviderKind::Codex, "sol".into(), Some("high".into())),
            ]
        );
        // The model kept for computer use leads there, and only there.
        assert_eq!(
            of("Computer use"),
            [
                (ProviderKind::Codex, "eyes".into(), Some("high".into())),
                (ProviderKind::Claude, "big".into(), Some("high".into())),
            ]
        );
        assert!(
            rows.iter()
                .filter(|r| r.model == "eyes")
                .all(|r| r.work == "Computer use")
        );
        // A fusion consult's kind is its own agent's only.
        assert_eq!(of("fusion-claude").len(), 1);
        assert_eq!(of("fusion-claude")[0].1, "big");
        // Nothing small is rated: the shipped rows stand in, as shipped.
        assert_eq!(
            of("Mechanical changes and search"),
            [(ProviderKind::Claude, "sonnet".into(), Some("medium".into()))]
        );
        let titles = rows
            .iter()
            .find(|r| r.work == "Conversation titles")
            .unwrap();
        assert!(!titles.reason.starts_with(RATED_REASON));
        // A model a rule or a switch keeps out gives its place to the agent's next best.
        let rows = automatic(
            super::rows(GUIDANCE),
            &models,
            &|model, _| model.id != "eyes",
            &|_| true,
        );
        assert_eq!(
            rows.iter()
                .filter(|row| row.work == "Computer use")
                .map(|row| row.model.as_str())
                .collect::<Vec<_>>(),
            ["big", "sol"]
        );
    }

    /// A better-rated model for a kind of work the user ordered is suggested; the user's rows
    /// are only read. Dismissed at its rating, it stays hidden until that rating changes.
    /// A role is compared with its own agent's best model only, under a kind's old name too:
    /// Nova beats Sol on Codex; it also rates above Big, but Big's role is Claude's. A role's
    /// dismissal is its own. Each agent's first row is the role.
    #[test]
    fn a_role_is_suggested_its_own_agents_better_model() {
        let (registry, mut models) = small_catalog();
        let rated = |model: &mut MergedModel, score: f64| {
            for category in brigadier_router::TaskCategory::ALL {
                model.strengths.insert(category, score);
            }
        };
        let sol = models.iter_mut().find(|m| m.id == "sol").unwrap();
        rated(sol, 8.0);
        let mut nova = sol.clone();
        nova.id = "nova".into();
        rated(&mut nova, 9.5);
        models.push(nova);
        rated(models.iter_mut().find(|m| m.id == "big").unwrap(), 9.0);
        let listed = |_: ProviderKind, word: &str| Some(word.to_owned());
        for work in [
            "Adversarial review",
            "Independent review",
            FALLBACK_WORK,
            "fusion-codex",
        ] {
            let mine = rows(&format!(
                "| {work} | claude | big | high | Mine. |\n| {work} | codex | sol | high | Mine. |\n| {work} | codex | sol | low | Older. |\n"
            ));
            let found = suggestions(&mine, &registry, &models, &|_, _| true, &listed, &[]);
            assert_eq!(found.len(), 1, "{work}: {found:?}");
            assert_eq!(
                (found[0].row.agent, found[0].row.model.as_str()),
                (ProviderKind::Codex, "nova"),
                "{work}"
            );
            assert_eq!(
                (
                    found[0].current.as_str(),
                    found[0].current_score,
                    found[0].score
                ),
                ("sol", 8.0, 9.5),
                "{work}"
            );
            let dismissed = [crate::DismissedSuggestion {
                work: found[0].work.clone(),
                agent: ProviderKind::Codex,
                model: "nova".into(),
                score: 9.5,
            }];
            assert!(
                suggestions(&mine, &registry, &models, &|_, _| true, &listed, &dismissed)
                    .is_empty(),
                "{work}"
            );
        }
    }
    /// Rule reading isn't rated: it keeps the shipped family word, which follows newer models.
    #[test]
    fn rule_reading_keeps_its_family_word() {
        let (_, models) = small_catalog();
        let shipped = automatic(rows(GUIDANCE), &models, &|_, _| true, &|_| true);
        let reading: Vec<_> = shipped
            .iter()
            .filter(|row| row.work == "Rule reading")
            .map(|row| (row.agent, row.model.as_str()))
            .collect();
        assert_eq!(
            reading,
            [
                (ProviderKind::Claude, "sonnet"),
                (ProviderKind::Codex, "default")
            ]
        );
    }
    #[test]
    fn a_suggestion_reads_the_users_order_and_never_changes_it() {
        let (registry, mut models) = small_catalog();
        let mine = rows(
            "| Computer use | claude | big | medium | Mine. |\n| Bounded implementation | codex | sol | high | Mine. |\n",
        );
        let kept = mine.clone();
        let listed = |_: ProviderKind, word: &str| Some(word.to_owned());
        let found = suggestions(&mine, &registry, &models, &|_, _| true, &listed, &[]);
        assert_eq!(mine, kept);
        // Eyes rates 9.6 for computer use, Big 9.0: suggested. Sol leads implementation already.
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].work, "Computer use");
        assert_eq!(
            (found[0].row.agent, found[0].row.model.as_str()),
            (ProviderKind::Codex, "eyes")
        );
        assert_eq!(
            (found[0].current.as_str(), found[0].current_score),
            ("big", 9.0)
        );
        let dismissed = [crate::DismissedSuggestion {
            work: "Computer use".into(),
            agent: ProviderKind::Codex,
            model: "eyes".into(),
            score: found[0].score,
        }];
        assert!(
            suggestions(&mine, &registry, &models, &|_, _| true, &listed, &dismissed).is_empty()
        );
        // Less than half a point higher: no suggestion.
        let eyes = models.iter_mut().find(|m| m.id == "eyes").unwrap();
        eyes.strengths
            .insert(brigadier_router::TaskCategory::Operate, 9.4);
        assert!(suggestions(&mine, &registry, &models, &|_, _| true, &listed, &[]).is_empty());
        // Its rating changed again: the dismissed suggestion is back.
        let eyes = models.iter_mut().find(|m| m.id == "eyes").unwrap();
        eyes.strengths
            .insert(brigadier_router::TaskCategory::Operate, 9.9);
        let found = suggestions(&mine, &registry, &models, &|_, _| true, &listed, &dismissed);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].score, 9.9);
        assert_eq!(mine, kept);
    }
    #[test]
    fn user_row_overrides_shipped_row() {
        let rows = guidance(
            rows(GUIDANCE),
            "| Bounded implementation | claude | sonnet | low | My preference. |",
        );
        let row = rows
            .iter()
            .find(|r| r.work == "Bounded implementation")
            .unwrap();
        assert_eq!(row.model, "sonnet");
        assert_eq!(row.source, "user");
    }
    /// #763: a hand-written `fallback` is the shipped `Fallback`, whatever its letter case: the
    /// user's order replaces the shipped rows in their place instead of landing after them.
    #[test]
    fn guidance_user_rows_replace_shipped_rows_whatever_the_case() {
        let shipped = rows(GUIDANCE);
        let user = "| fallback | codex | sol | | Mine. |\n| BOUNDED implementation | claude | sonnet | low | Mine. |\n";
        let merged = guidance(shipped.clone(), user);
        for work in [FALLBACK_WORK, "Bounded implementation"] {
            let of: Vec<_> = merged
                .iter()
                .filter(|row| same_work(&row.work, work))
                .collect();
            assert_eq!(of.len(), 1, "{work}: only the user's row, {merged:#?}");
            assert_eq!(of[0].source, "user");
        }
        // Each kind once, in the shipped order: neither user kind went to the end.
        let kinds = |works: Vec<String>| {
            works
                .into_iter()
                .fold(Vec::<String>::new(), |mut kinds, work| {
                    if !kinds.iter().any(|kind| same_work(kind, &work)) {
                        kinds.push(work.to_ascii_lowercase());
                    }
                    kinds
                })
        };
        assert_eq!(
            kinds(merged.into_iter().map(|row| row.work).collect()),
            kinds(shipped.into_iter().map(|row| row.work).collect()),
        );
    }
    /// #762: a line that can't be a rule is left out and named; the rules around it still hold,
    /// and only an explicit action (`checked`) stops on it.
    #[test]
    fn policy_keeps_good_lines_and_reports_a_bad_one() {
        let policy = Policy::parse(
            "# Mine
- forbid: fable
- effort-ceiling: hgih
- effort-ceiling: high
",
        );
        assert_eq!(policy.invalid, ["line 3: - effort-ceiling: hgih"]);
        assert_eq!(policy.entries.len(), 2);
        assert!(
            policy
                .refusal(ProviderKind::Claude, &["claude-fable-5-1"], None, None)
                .is_some()
        );
        assert_eq!(
            policy.effort(ProviderKind::Claude, &["opus"], Some("max"), &[], None),
            Ok(Some("high".into()))
        );
        let refused = policy.checked().err().unwrap().to_string();
        assert!(
            refused.contains("line 3: - effort-ceiling: hgih"),
            "{refused}"
        );
        assert!(
            Policy::parse(
                "- forbid: fable
"
            )
            .checked()
            .is_ok()
        );
    }
    #[test]
    fn the_old_review_name_reads_as_adversarial_review() {
        let old = "Independent review";
        let current = "Adversarial review";
        assert_eq!(canonical_work(old), current);
        assert!(same_work(" independent review ", current));
        assert!(same_work(old, " adversarial REVIEW "));

        let user = format!("| {old} | claude | sonnet | medium | My reviewer. |\n");
        let rows = guidance(rows(GUIDANCE), &user);
        let review: Vec<_> = rows.iter().filter(|r| same_work(&r.work, old)).collect();
        assert_eq!(
            review.len(),
            1,
            "the old row replaces the shipped reviewers"
        );
        assert_eq!(review[0].work, current);
        assert_eq!(review[0].model, "sonnet");
        assert_eq!(review[0].source, "user");

        let policy = Policy::parse(&format!(
            "- forbid for {old}: sonnet\n- effort-ceiling for {old}: medium\n"
        ));
        for (_, rule) in &policy.entries {
            assert_eq!(rule.work(), Some(current));
        }
        for work in [old, current] {
            assert!(
                policy
                    .refusal(ProviderKind::Claude, &["sonnet"], None, Some(work))
                    .unwrap()
                    .contains(old),
                "a refusal still quotes the user's rule verbatim"
            );
            assert_eq!(
                policy.effort(
                    ProviderKind::Claude,
                    &["opus"],
                    Some("high"),
                    &[],
                    Some(work)
                ),
                Ok(Some("medium".into()))
            );
            assert!(
                policy
                    .refusal(ProviderKind::Claude, &["opus"], Some("high"), Some(work))
                    .is_none()
            );
        }
    }
    #[test]
    fn a_works_order_replaces_its_shipped_row_in_place() {
        let user = "| Adversarial review | claude | opus | high | First. |\n| Adversarial review | codex | gpt-6 | | Next. |\n| Docs | claude | haiku | low | Mine. |\n";
        let rows = guidance(rows(GUIDANCE), user);
        let works: Vec<_> = rows
            .iter()
            .map(|r| (r.work.as_str(), r.model.as_str(), r.source.as_str()))
            .collect();
        assert_eq!(
            &works[3..6],
            [
                ("Adversarial review", "opus", "user"),
                ("Adversarial review", "gpt-6", "user"),
                ("fusion-claude", "opus", "shipped"),
            ]
        );
        assert_eq!(works.last(), Some(&("Docs", "haiku", "user")));
        assert_eq!(
            rows.iter()
                .filter(|r| r.work == "Adversarial review")
                .count(),
            2
        );
    }
    #[test]
    fn a_rule_for_one_kind_of_work_judges_only_its_starts() {
        let policy = Policy::parse(
            "- forbid for Adversarial review: sonnet\n- effort-ceiling for Docs: low\n",
        );
        assert_eq!(policy.entries[0].0, "forbid for Adversarial review: sonnet");
        assert_eq!(
            policy.entries[0].1,
            ModelRule::Forbid {
                names: vec!["sonnet".into()],
                work: Some("Adversarial review".into())
            }
        );
        let sonnet = ["claude-sonnet-5-5"];
        assert!(
            policy
                .refusal(
                    ProviderKind::Claude,
                    &sonnet,
                    None,
                    Some("adversarial review")
                )
                .unwrap()
                .contains("forbid for Adversarial review: sonnet")
        );
        assert!(
            policy
                .refusal(ProviderKind::Claude, &sonnet, None, None)
                .is_none()
        );
        assert!(
            policy
                .refusal(ProviderKind::Claude, &sonnet, None, Some("Docs"))
                .is_none()
        );
        // D-L38: an effort above a cap runs at the cap; it is not refused.
        assert!(
            policy
                .refusal(ProviderKind::Claude, &[], Some("medium"), Some("Docs"))
                .is_none()
        );
        assert_eq!(
            policy.effort(ProviderKind::Claude, &[], Some("medium"), &[], Some("Docs")),
            Ok(Some("low".into()))
        );
        assert_eq!(
            policy.effort(ProviderKind::Claude, &[], Some("medium"), &[], None),
            Ok(Some("medium".into()))
        );
        // A chat or session, and a CLI's own subagents, are judged by rules for every work only.
        assert!(!policy.restricts_models(ProviderKind::Claude));
    }
    #[test]
    fn shipped_default_has_no_rules() {
        let policy = Policy::parse(DEFAULT_RULES);
        assert!(policy.entries.is_empty());
        for provider in brigadier_providers::adapters::kinds() {
            assert!(!policy.restricts_models(provider));
            assert!(
                policy
                    .refusal(
                        provider,
                        &["claude-fable-5-1", "any-model"],
                        Some("ultra"),
                        None
                    )
                    .is_none()
            );
        }
    }
    #[test]
    fn user_rules_are_displayed_and_quote_the_rule() {
        let policy = Policy::parse("- forbid: fable\n- effort-ceiling: high\n");
        assert_eq!(policy.entries[0].0, "forbid: fable");
        assert!(
            policy
                .refusal(ProviderKind::Claude, &["claude-fable-5-1"], None, None)
                .unwrap()
                .contains("forbid: fable")
        );
        assert!(
            Policy::parse("")
                .refusal(
                    ProviderKind::Claude,
                    &["claude-fable-5-1"],
                    Some("ultra"),
                    None
                )
                .is_none()
        );
        assert_eq!(
            policy.effort(ProviderKind::Codex, &[], Some("xhigh"), &[], None),
            Ok(Some("high".into()))
        );
        let policy = Policy::parse("- allow claude: opus, sonnet");
        assert!(
            policy
                .refusal(ProviderKind::Codex, &["other"], None, None)
                .is_none()
        );
        assert!(
            policy
                .refusal(ProviderKind::Claude, &["haiku"], None, None)
                .is_some()
        );
    }
    #[test]
    fn allowlists_accept_a_reported_alias_and_denials_inspect_its_resolution() {
        let names = ["opus", "claude-opus-5-5"];
        assert!(
            Policy::parse("- allow claude: claude-opus-5-5")
                .refusal(ProviderKind::Claude, &names, None, None)
                .is_none()
        );
        assert!(
            Policy::parse("- forbid: fable")
                .refusal(
                    ProviderKind::Claude,
                    &["opus", "claude-fable-5-1"],
                    None,
                    None
                )
                .is_some()
        );
    }
    #[test]
    fn stale_routing_save_keeps_newer_rows_rules_and_advice() {
        let dir = std::env::temp_dir().join(format!("routing-stale-save-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("models.md");
        let first = editable(&path, DEFAULT_RULES).unwrap();
        let revision = rules_revision(&first);
        let row = GuidanceRow {
            work: "Docs".into(),
            agent: ProviderKind::Claude,
            model: "sonnet".into(),
            effort: Some("medium".into()),
            reason: "Mine".into(),
        };
        let rules = [
            ModelRule::Forbid {
                names: vec!["opus".into()],
                work: None,
            },
            ModelRule::Advice {
                text: "Use cheap models when busy".into(),
            },
        ];
        save_rules(&path, &revision, &[row], &rules).unwrap();
        let newer = std::fs::read_to_string(&path).unwrap();
        let err = save_rules(&path, &revision, &[], &[]).unwrap_err();
        assert!(err.to_string().contains("Routing changed since"), "{err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), newer);
        // A hand edit to prose alone also invalidates the page's revision.
        std::fs::write(&path, format!("{newer}Hand-edited advice.\n")).unwrap();
        assert!(save_rules(&path, &rules_revision(&newer), &[], &[]).is_err());
        let current = std::fs::read_to_string(&path).unwrap();
        save_rules(&path, &rules_revision(&current), &[], &[]).unwrap();
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("Hand-edited advice.")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn advice_with_pipes_is_never_a_routing_row() {
        let rule = ModelRule::Advice {
            text: "Prefer cheap models | claude | opus | high | use when busy".into(),
        };
        let written = rewrite(DEFAULT_RULES, &[], std::slice::from_ref(&rule));
        assert!(
            rows(&written).is_empty(),
            "advice created routing rows: {:?}",
            rows(&written)
        );
        assert_eq!(Policy::parse(&written).entries[0].1, rule);
    }

    #[test]
    fn saving_keeps_unrelated_tables_and_unreadable_routing_rows() {
        let notes = "| Preference | Reason |\n| --- | --- |\n| Cheap | Save quota |\n\n| Name | Provider | Model | Effort | Note |\n| --- | --- | --- | --- | --- |\n| Note | claude | opus | high | Only a note |\n";
        let future = "| Docs | future-agent | cheap | medium | Keep my future agent |\n";
        let text = format!(
            "# Mine\n{notes}\n{TABLE_HEAD}\n{future}| Docs | claude | sonnet | low | Current |\n"
        );
        let written = rewrite(&text, &[], &[]);
        assert!(written.contains(notes), "lost advice notes: {written}");
        assert!(rows(&written).is_empty(), "notes became routing rows");
        assert!(
            written.contains(future),
            "lost an unreadable user row: {written}"
        );
        assert!(!written.contains("| Docs | claude |"));
        assert_eq!(rewrite(&written, &[], &[]), written);
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
                work: None,
            },
            ModelRule::Allow {
                agent: Some(ProviderKind::Codex),
                names: vec!["gpt-6".into(), "default".into()],
                work: None,
            },
            ModelRule::EffortCeiling {
                effort: "high".into(),
                agent: None,
                model: None,
                work: None,
            },
            ModelRule::Forbid {
                names: vec!["sonnet".into()],
                work: Some("Adversarial review".into()),
            },
        ];
        check_rules(std::slice::from_ref(&row), &rules).unwrap();
        // The shipped file has prose only: rules and the table go after it.
        let written = rewrite(DEFAULT_RULES, std::slice::from_ref(&row), &rules);
        assert!(written.starts_with(DEFAULT_RULES.trim_end()));
        assert!(written.contains(
            "- forbid: fable\n- allow codex: gpt-6, default\n- effort-ceiling: high\n- forbid for Adversarial review: sonnet\n\n| Work |"
        ));
        assert!(
            written.ends_with(
                "| Bounded implementation | claude | sonnet | medium | My preference. |\n"
            )
        );
        // Read back, it is what was written; written again, it is the same text.
        let policy = Policy::parse(&written);
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
        // A work's order may hold several models, each once.
        let next = GuidanceRow {
            agent: ProviderKind::Codex,
            model: "gpt-6".into(),
            ..row.clone()
        };
        assert!(check_rules(&[row.clone(), next], &[]).is_ok());
        assert!(check_rules(&[row.clone(), row], &[]).is_err());
        assert!(
            check_rules(
                &[],
                &[ModelRule::Forbid {
                    names: vec![],
                    work: None
                }]
            )
            .is_err()
        );
        assert!(
            check_rules(
                &[],
                &[ModelRule::EffortCeiling {
                    effort: "huge".into(),
                    agent: None,
                    model: None,
                    work: None
                }]
            )
            .is_err()
        );
        assert!(
            check_rules(
                &[],
                &[ModelRule::Forbid {
                    names: vec!["sonnet".into()],
                    work: Some("Docs: short".into())
                }]
            )
            .is_err()
        );
    }
    fn effort(
        policy: &Policy,
        provider: ProviderKind,
        models: &[&str],
        asked: Option<&str>,
        work: Option<&str>,
    ) -> std::result::Result<Option<String>, String> {
        policy.effort(provider, models, asked, &[], work)
    }
    #[test]
    fn an_effort_cap_for_one_model_moves_only_that_model() {
        let policy = Policy::parse("- effort-ceiling opus: medium\n");
        assert_eq!(
            policy.entries[0].1,
            ModelRule::EffortCeiling {
                effort: "medium".into(),
                agent: None,
                model: Some("opus".into()),
                work: None,
            }
        );
        let opus = ["opus", "claude-opus-5-5"];
        assert_eq!(
            effort(&policy, ProviderKind::Claude, &opus, Some("max"), None),
            Ok(Some("medium".into()))
        );
        assert_eq!(
            effort(&policy, ProviderKind::Claude, &opus, Some("low"), None),
            Ok(Some("low".into()))
        );
        assert_eq!(
            effort(
                &policy,
                ProviderKind::Claude,
                &["claude-sonnet-5-5"],
                Some("max"),
                None
            ),
            Ok(Some("max".into()))
        );
        assert!(
            policy
                .refusal(ProviderKind::Claude, &opus, Some("max"), None)
                .is_none()
        );
        // No effort named and no minimum: the model's own default stays.
        assert_eq!(
            effort(&policy, ProviderKind::Claude, &opus, None, None),
            Ok(None)
        );
    }
    #[test]
    fn an_effort_cap_for_one_agent_moves_only_that_agent() {
        let policy = Policy::parse("- effort-ceiling codex: high\n");
        assert_eq!(
            policy.entries[0].1,
            ModelRule::EffortCeiling {
                effort: "high".into(),
                agent: Some(ProviderKind::Codex),
                model: None,
                work: None,
            }
        );
        assert_eq!(
            effort(
                &policy,
                ProviderKind::Codex,
                &["gpt-6.1-sol"],
                Some("ultra"),
                None
            ),
            Ok(Some("high".into()))
        );
        assert_eq!(
            effort(&policy, ProviderKind::Claude, &["opus"], Some("max"), None),
            Ok(Some("max".into()))
        );
        // The lowest cap that holds wins.
        let policy = Policy::parse("- effort-ceiling: xhigh\n- effort-ceiling codex: high\n");
        assert_eq!(
            effort(&policy, ProviderKind::Codex, &[], Some("max"), None),
            Ok(Some("high".into()))
        );
        assert_eq!(
            effort(&policy, ProviderKind::Claude, &[], Some("max"), None),
            Ok(Some("xhigh".into()))
        );
    }
    #[test]
    fn a_minimum_effort_brings_less_up() {
        let policy = Policy::parse("- effort-floor for Adversarial review: high\n");
        assert_eq!(
            policy.entries[0].1,
            ModelRule::EffortFloor {
                effort: "high".into(),
                agent: None,
                model: None,
                work: Some("Adversarial review".into()),
            }
        );
        let review = Some("Adversarial review");
        assert_eq!(
            effort(
                &policy,
                ProviderKind::Claude,
                &["opus"],
                Some("low"),
                review
            ),
            Ok(Some("high".into()))
        );
        // No effort named: a minimum names one.
        assert_eq!(
            effort(&policy, ProviderKind::Claude, &["opus"], None, review),
            Ok(Some("high".into()))
        );
        assert_eq!(
            effort(
                &policy,
                ProviderKind::Claude,
                &["opus"],
                Some("max"),
                review
            ),
            Ok(Some("max".into()))
        );
        // Other work, a chat: not this rule's.
        assert_eq!(
            effort(&policy, ProviderKind::Claude, &["opus"], Some("low"), None),
            Ok(Some("low".into()))
        );
        // With the model's efforts listed, the nearest of them within the rules.
        let listed: Vec<String> = ["low", "medium", "max"].map(String::from).to_vec();
        assert_eq!(
            policy.effort(
                ProviderKind::Claude,
                &["opus"],
                Some("low"),
                &listed,
                review
            ),
            Ok(Some("max".into()))
        );
        let capped = Policy::parse("- effort-ceiling: low\n");
        let only_high: Vec<String> = ["high", "max"].map(String::from).to_vec();
        assert!(
            capped
                .effort(
                    ProviderKind::Claude,
                    &["deepseek"],
                    Some("max"),
                    &only_high,
                    None
                )
                .unwrap_err()
                .contains("effort-ceiling: low"),
            "no effort of the model is within the cap"
        );
    }
    #[test]
    fn effort_rules_adjust_and_refuse_only_when_two_cannot_both_be_met() {
        let policy = Policy::parse(
            "- effort-ceiling opus: medium\n- effort-floor for Adversarial review: high\n",
        );
        let opus = ["opus", "claude-opus-5-5"];
        // Each alone moves the effort.
        assert_eq!(
            effort(&policy, ProviderKind::Claude, &opus, Some("max"), None),
            Ok(Some("medium".into()))
        );
        assert_eq!(
            effort(
                &policy,
                ProviderKind::Codex,
                &["gpt-6.1-sol"],
                Some("low"),
                Some("Adversarial review")
            ),
            Ok(Some("high".into()))
        );
        // Together, on Opus for a review: refused, quoting both.
        let refusal = policy
            .refusal(
                ProviderKind::Claude,
                &opus,
                Some("high"),
                Some("Adversarial review"),
            )
            .unwrap();
        assert_eq!(
            refusal,
            "refused by your model rules: \"effort-ceiling opus: medium\" and \"effort-floor for Adversarial review: high\" can't both be met"
        );
        assert!(
            policy
                .refusal(
                    ProviderKind::Claude,
                    &opus,
                    None,
                    Some("Adversarial review")
                )
                .is_some()
        );
    }
    #[test]
    fn advice_notes_round_trip_and_never_refuse() {
        let rules = vec![
            ModelRule::EffortFloor {
                effort: "high".into(),
                agent: None,
                model: None,
                work: Some("Adversarial review".into()),
            },
            ModelRule::EffortCeiling {
                effort: "medium".into(),
                agent: None,
                model: Some("opus".into()),
                work: None,
            },
            ModelRule::EffortCeiling {
                effort: "high".into(),
                agent: Some(ProviderKind::Claude),
                model: None,
                work: Some("Docs".into()),
            },
            ModelRule::Advice {
                text: "prefer Codex when Claude is busy: it is faster".into(),
            },
        ];
        check_rules(&[], &rules).unwrap();
        let written = rewrite(DEFAULT_RULES, &[], &rules);
        assert!(written.ends_with(
            "- effort-floor for Adversarial review: high\n- effort-ceiling opus: medium\n- effort-ceiling claude for Docs: high\n- advice: prefer Codex when Claude is busy: it is faster\n"
        ), "{written}");
        let policy = Policy::parse(&written);
        assert_eq!(
            policy
                .entries
                .iter()
                .map(|(_, r)| r.clone())
                .collect::<Vec<_>>(),
            rules
        );
        assert_eq!(
            policy.advice(),
            ["prefer Codex when Claude is busy: it is faster"]
        );
        assert_eq!(rewrite(&written, &[], &rules), written);
        for provider in brigadier_providers::adapters::kinds() {
            assert!(!policy.restricts_models(provider));
        }
        let only_advice = Policy::parse("- advice: never use fable\n");
        assert!(
            only_advice
                .refusal(
                    ProviderKind::Claude,
                    &["claude-fable-5-1"],
                    Some("ultra"),
                    None
                )
                .is_none()
        );
        // The page never writes what the file cannot hold.
        for bad in [
            ModelRule::Advice { text: " ".into() },
            ModelRule::Advice {
                text: "two\nlines".into(),
            },
            ModelRule::EffortCeiling {
                effort: "high".into(),
                agent: Some(ProviderKind::Claude),
                model: Some("opus".into()),
                work: None,
            },
            ModelRule::EffortFloor {
                effort: "high".into(),
                agent: None,
                model: Some("two words".into()),
                work: None,
            },
            ModelRule::EffortFloor {
                effort: "high".into(),
                agent: None,
                model: Some("codex".into()),
                work: None,
            },
        ] {
            assert!(
                check_rules(&[], std::slice::from_ref(&bad)).is_err(),
                "{bad:?}"
            );
        }
    }
    #[test]
    fn an_old_rules_file_reads_as_it_did() {
        // The shape of a file from before the new rules: prose, a scoped ceiling, notes with colons.
        let old = "# My model rules\n\nEffort levels: none, minimal, low.\nA rule may name one Work (`forbid for Docs: sonnet`).\n- forbid: fable\n- allow claude: opus, sonnet\n- effort-ceiling: high\n- effort-ceiling for Independent review: medium\nnote: keep it cheap\n";
        let policy = Policy::parse(old);
        assert_eq!(
            policy
                .entries
                .iter()
                .map(|(_, r)| r.clone())
                .collect::<Vec<_>>(),
            vec![
                ModelRule::Forbid {
                    names: vec!["fable".into()],
                    work: None
                },
                ModelRule::Allow {
                    agent: Some(ProviderKind::Claude),
                    names: vec!["opus".into(), "sonnet".into()],
                    work: None
                },
                ModelRule::EffortCeiling {
                    effort: "high".into(),
                    agent: None,
                    model: None,
                    work: None
                },
                ModelRule::EffortCeiling {
                    effort: "medium".into(),
                    agent: None,
                    model: None,
                    work: Some("Adversarial review".into())
                },
            ]
        );
        assert!(policy.advice().is_empty());
        // Its JSON from an older page (no agent or model on a ceiling) still reads.
        let rule: ModelRule =
            serde_json::from_str(r#"{"type":"effortCeiling","effort":"high"}"#).unwrap();
        assert_eq!(
            rule,
            ModelRule::EffortCeiling {
                effort: "high".into(),
                agent: None,
                model: None,
                work: None
            }
        );
        assert_eq!(
            serde_json::to_string(&rule).unwrap(),
            r#"{"type":"effortCeiling","effort":"high"}"#
        );
        // The shipped prose holds no directive.
        assert!(Policy::parse(DEFAULT_RULES).entries.is_empty());
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
        let now = 0;
        assert_eq!(fit(&[], None, now), "unknown");
        assert_eq!(fit(&[], Some(None), now), "out");
        assert_eq!(fit(&[window.clone()], None, now), "ok");
        window.runway_ms = Some(1000);
        assert_eq!(fit(&[window.clone()], None, now), "tight");
        window.used_percent = 100.0;
        assert_eq!(
            fit(&[window.clone()], None, now),
            "out until 1970-01-01 01:00 UTC (in 1h)"
        );
        window.pooled = false;
        assert_eq!(
            fit(&[window.clone()], None, now),
            "ok",
            "a scoped window does not exhaust every model"
        );
        assert_eq!(
            fit(&[window], Some(Some(3_600_000)), now),
            "out until 1970-01-01 01:00 UTC (in 1h)"
        );
    }
    /// #764: a weekly window out for days names its reset's date and how far away it is, so a
    /// thread choosing a worker's account can't take it for hours away.
    #[test]
    fn fit_dates_a_reset_days_away() {
        // 2026-11-04 14:00 UTC; the reset five days later at 07:00.
        let now = 1_793_800_800_000;
        let reset = now + 4 * 86_400_000 + 17 * 3_600_000;
        let window = Usage {
            window: "seven_day".into(),
            used_percent: 100.0,
            resets_at_ms: Some(reset),
            model: None,
            bucket: None,
            pooled: true,
            runway_ms: None,
            estimate_rough: false,
        };
        assert_eq!(
            fit(&[window], None, now),
            "out until 2026-11-09 07:00 UTC (in 4d)"
        );
        assert_eq!(
            fit(&[], Some(Some(now + 3 * 3_600_000)), now),
            "out until 2026-11-04 17:00 UTC (in 3h)"
        );
    }
}
