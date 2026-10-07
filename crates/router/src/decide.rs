//! Routing v2: choosing a model for one task from the merged catalog, the quota view, outcomes
//! and the user's rules. See the crate docs for the design; this module is its implementation.

use std::borrow::Cow;

use brigadier_providers::{LimitKind, ProviderKind};

use crate::explain::{Alternative, Explanation, Factor, RouteCandidate};
use crate::forecast::{LIMITED_USED, active_limit, quota_penalty, window_applies};
use crate::learn::{is_model, learned_for};
use crate::load::clamp_effort;
use crate::merge::from_entry;
use crate::outcome::Learned;
use crate::overrides::{OverrideEffect, OverrideRule, OverrideTarget};
use crate::quota::{Heat, ProviderQuota};
use crate::rankings::{RankedPlace, Ranking, RankingUse, ranking_for, ranking_text, target_text};
use crate::registry::{Capability, MergedModel, Modality, ModelStatus, QualityTier, Registry};
use crate::{Area, Author, Pin, TaskCategory, name, table};

/// Added to a trial model's score on a task holding a trial slot.
pub const TRIAL_BONUS: f64 = 4.0;
/// Taken from a model's score per worker already running on its provider.
pub const LOAD_PENALTY: f64 = 0.25;
pub const MAX_LOAD_PENALTY: f64 = 1.5;
/// Taken from an older model the CLI keeps beside a newer one.
pub const LEGACY_PENALTY: f64 = 1.0;
/// Taken from a model that inherits a family entry until it has outcomes here.
pub const INHERITED_PENALTY: f64 = 0.25;
/// Outcomes after which an inherited model no longer pays [`INHERITED_PENALTY`].
const INHERITED_PROVEN: u32 = 3;
/// The context window assumed for a model whose window nobody knows.
pub const ASSUMED_CONTEXT_WINDOW: i64 = 128_000;
/// Scores this close count as a tie.
const TIE: f64 = 1e-6;
/// The reason sentence stops adding notes past this length.
const REASON_BUDGET: usize = 180;

/// What a task needs from its model.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Needs {
    /// It sends images.
    pub image_input: bool,
    /// It makes images (Codex only).
    pub image_generation: bool,
    /// The context it needs, in tokens (a hand-off after a context-window error asks for more
    /// than the failed model had).
    pub context_tokens: Option<i64>,
}

/// A provider or one model that must not take the task (it just failed on it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exclusion {
    pub provider: ProviderKind,
    /// One model (its id or concrete model); `None`: every model of the provider.
    pub model: Option<String>,
}

/// One provider as routing sees it now.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderState {
    pub provider: ProviderKind,
    /// Installed and logged in.
    pub logged_in: bool,
    /// The quota monitor's view; `None` before the first read.
    pub quota: Option<ProviderQuota>,
}

/// One routing question (routing v2).
#[derive(Debug, Clone)]
pub struct Query<'a> {
    pub category: TaskCategory,
    pub areas: &'a [Area],
    /// The least capable model it may run on (see [`default_floor`]).
    pub floor: QualityTier,
    pub needs: Needs,
    /// A provider, model or effort asked for by the orchestrator or the user.
    pub pin: Option<Pin>,
    /// The pin binds (a hand-off or a resume): only the pinned vendor may take the task (the
    /// named model's vendor when the pin names only a model), and with none of its models
    /// available the task waits. Otherwise a pin that can't be met gives way to routing, and
    /// the reason says so.
    pub hold_pin: bool,
    /// For reviews: the model that wrote the change.
    pub avoid: Option<Author>,
    /// For a second reviewer or checker: the models already checking the change. Another
    /// model takes it when one can, even of the author's vendor (never the author's model).
    pub distinct_from: Vec<Author>,
    /// Providers or models that just failed on this task.
    pub exclude: &'a [Exclusion],
    /// The user's rules (global and per project); those for another project, category or area
    /// are skipped here.
    pub overrides: &'a [OverrideRule],
    /// The user's manual rankings (global and per project); the one that applies is picked
    /// here ([`ranking_for`]).
    pub rankings: &'a [Ranking],
    pub project_id: Option<&'a str>,
    /// Workers each provider runs now.
    pub running: &'a [(ProviderKind, u32)],
    /// This task may go to a model on trial (the caller gives at most 1 in 5 low-risk tasks a
    /// slot).
    pub trial_slot: bool,
    pub providers: &'a [ProviderState],
    /// The merged catalog ([`crate::merge`]), every provider, in each CLI's order.
    pub models: &'a [MergedModel],
    pub registry: &'a Registry,
    /// What this project's outcomes taught ([`crate::learn`]).
    pub learned: &'a [Learned],
    pub now_ms: i64,
}

/// Where a task runs, and why.
#[derive(Debug, Clone, PartialEq)]
pub struct Routed {
    pub provider: ProviderKind,
    /// The model id to pass to the CLI.
    pub model: String,
    /// The reasoning effort; `None` for the model's default (or a model without effort levels).
    pub effort: Option<String>,
    /// One short sentence for the worker card.
    pub reason: String,
    pub explanation: Explanation,
    /// For reviews: whether the reviewer's vendor differs from the author's.
    pub cross_vendor: Option<bool>,
    /// It runs as a trial of a new model.
    pub trial: bool,
    pub tier: QualityTier,
}

/// Nothing the task may use is available: it waits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Waiting {
    /// What it waits for ("Claude's 5-hour window is used up (resets in 2h 10m)").
    pub reason: String,
    /// The earliest reset that could let it run, when known.
    pub resets_at_ms: Option<i64>,
    /// The user rule that keeps it from other models, if one does (its text).
    pub rule: Option<String>,
    /// The user's ranking that keeps it from other models (Only these), if one does (its
    /// text).
    pub ranking: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    Run(Routed),
    Wait(Waiting),
}

/// What routing would do, with every model it weighed: the Routing page's live order.
#[derive(Debug, Clone, PartialEq)]
pub struct Preview {
    pub decision: Decision,
    /// The chosen model first, then the others in the order routing would try them, then
    /// those that can't take the task.
    pub candidates: Vec<RouteCandidate>,
    /// The manual ranking in force, place by place; empty when routing scores.
    pub places: Vec<RankedPlace>,
}

/// The quality floor a task of this category gets unless the orchestrator sets one: code
/// writing and review run on a vendor's best, chores on a small model.
pub fn default_floor(category: TaskCategory) -> QualityTier {
    match category {
        TaskCategory::Implement | TaskCategory::Review | TaskCategory::Merge => {
            QualityTier::Frontier
        }
        TaskCategory::Orchestrate => QualityTier::Strong,
        TaskCategory::Research => QualityTier::Standard,
        TaskCategory::Scout | TaskCategory::Verify | TaskCategory::Chat => QualityTier::Light,
    }
}

/// Whether a category is low-risk work a model on trial may take.
pub fn allows_trials(category: TaskCategory) -> bool {
    matches!(
        category,
        TaskCategory::Scout | TaskCategory::Research | TaskCategory::Verify
    )
}

/// A category's default effort, fitted to its floor: a verifier that runs at low effort tends
/// to call checks met that it never ran, and code is written and reviewed at high effort. A
/// scout is a chore and runs at low.
fn at_least(category: TaskCategory, effort: &'static str) -> &'static str {
    let floor = match category {
        TaskCategory::Verify => "medium",
        TaskCategory::Implement | TaskCategory::Review | TaskCategory::Merge => "high",
        TaskCategory::Scout => return "low",
        _ => return effort,
    };
    if table::rank(effort) < table::rank(floor) {
        floor
    } else {
        effort
    }
}

/// The effort a category runs at when the registry names none.
fn category_effort(category: TaskCategory) -> Option<&'static str> {
    match category {
        TaskCategory::Scout => Some("low"),
        TaskCategory::Research | TaskCategory::Verify | TaskCategory::Orchestrate => Some("medium"),
        TaskCategory::Implement | TaskCategory::Review | TaskCategory::Merge => Some("high"),
        TaskCategory::Chat => None,
    }
}

// ----- candidates ----------------------------------------------------------------------------

/// Why a model can't take the task, in the order they are checked.
#[derive(Debug, Clone, PartialEq)]
enum Block {
    /// It (or its provider) just failed on this task.
    Excluded(String),
    /// A user rule removes it (`never`, or outside every `only`).
    Rule(String),
    /// It can't do what the task needs.
    Needs(String),
    /// Below the task's quality floor.
    Floor(String),
    /// Its provider isn't logged in.
    Unavailable(String),
    /// A confirmed limit: a hard exclusion until the reset.
    Limit {
        why: String,
        resets_at_ms: Option<i64>,
    },
    /// The review's author vendor, while another vendor can review.
    Author(String),
    /// Another vendor than the one a binding pin asked for.
    Pinned(String),
}

impl Block {
    fn why(&self) -> &str {
        match self {
            Block::Excluded(why)
            | Block::Rule(why)
            | Block::Needs(why)
            | Block::Floor(why)
            | Block::Unavailable(why)
            | Block::Author(why)
            | Block::Pinned(why)
            | Block::Limit { why, .. } => why,
        }
    }
}

struct Candidate<'q> {
    model: Cow<'q, MergedModel>,
    /// Built from the registry: its CLI's list isn't loaded yet.
    unchecked: bool,
    trial: bool,
    block: Option<Block>,
    base: f64,
    factors: Vec<Factor>,
    score: f64,
    /// The quota penalty in `score`, and the window behind it.
    penalty: f64,
    penalty_window: Option<String>,
    /// The load penalty in `score`, and what it says.
    load: f64,
    load_note: Option<String>,
    learned: Option<&'q Learned>,
    /// A preferred target of an applicable `prefer` rule (its text).
    preferred: Option<String>,
    /// Its first place in the manual ranking in force (1-based).
    listed: Option<u32>,
}

impl Candidate<'_> {
    fn eligible(&self) -> bool {
        self.block.is_none()
    }

    fn label(&self) -> String {
        format!("{} {}", name(self.model.provider), self.model.id)
    }
}

/// Routes one task. See the crate docs for the rules.
///
/// **Fallback** is this same call with the failed provider or model in [`Query::exclude`] and
/// the task's floor, areas and rules unchanged: nothing below the floor is ever taken, and a
/// user rule holds during fallback too (an `only` rule whose models are all unavailable makes the
/// task wait, naming the rule).
pub fn decide(query: &Query) -> Decision {
    preview(query).decision
}

/// Every model that may take the task, by the same checks [`decide`] runs: no Fable, a login
/// and no confirmed limit, not just failed on it, the user's rules and rankings, its needs and
/// quality floor, a binding pin, and for reviews the author's vendor or model. The model
/// [`decide`] picks is always among them; empty when the task would wait. What the task's own
/// sub-agents may run on.
pub fn eligible(query: &Query) -> Vec<MergedModel> {
    assessed(query)
        .candidates
        .into_iter()
        .filter(Candidate::eligible)
        .map(|candidate| candidate.model.into_owned())
        .collect()
}

/// Every candidate checked and scored, with a review's author and a second checker's peers
/// kept out: what [`preview`] orders and picks from, and [`eligible`] lists.
struct Assessed<'q> {
    rules: Vec<&'q OverrideRule>,
    only: Vec<&'q OverrideRule>,
    ranking: Option<&'q Ranking>,
    held: Option<ProviderKind>,
    candidates: Vec<Candidate<'q>>,
    notes: Vec<String>,
    cross_vendor: Option<bool>,
}

fn assessed<'q>(query: &'q Query) -> Assessed<'q> {
    let rules: Vec<&OverrideRule> = query
        .overrides
        .iter()
        .filter(|rule| applies(rule, query))
        .collect();
    let only: Vec<&OverrideRule> = rules
        .iter()
        .copied()
        .filter(|rule| rule.effect == OverrideEffect::Only)
        .collect();
    // A manual ranking with at least one place decides; an empty one leaves it to scores,
    // unless it allows only its models: then nothing may run.
    let ranking = ranking_for(
        query.rankings,
        query.category,
        query.areas,
        query.project_id,
    )
    .filter(|ranking| ranking.manual && (!ranking.entries.is_empty() || ranking.only));
    let mut candidates = candidates(query);
    if let Some(ranking) = ranking {
        for candidate in &mut candidates {
            candidate.listed = ranking
                .entries
                .iter()
                .position(|entry| targets(&entry.target, &candidate.model, query.registry))
                .map(|index| index as u32 + 1);
        }
    }
    let held = held_vendor(query, &candidates);
    for candidate in &mut candidates {
        assess(candidate, query, &rules, &only, held, ranking);
    }

    // Reviews: another vendor when one can take it, else a different model of the same one.
    let mut notes: Vec<String> = Vec::new();
    let mut cross_vendor = None;
    if let Some(author) = &query.avoid {
        let other_vendor = candidates
            .iter()
            .any(|c| c.eligible() && c.model.provider != author.provider);
        if other_vendor {
            cross_vendor = Some(true);
            for candidate in &mut candidates {
                if candidate.eligible() && candidate.model.provider == author.provider {
                    candidate.block = Some(Block::Author(format!(
                        "{} wrote the change under review",
                        name(author.provider)
                    )));
                }
            }
        } else if candidates.iter().any(|c| c.eligible()) {
            cross_vendor = Some(false);
            let is_author = |c: &Candidate| {
                author
                    .model
                    .as_deref()
                    .is_some_and(|model| is_model(&c.model, query.registry, model))
            };
            let others = candidates.iter().any(|c| c.eligible() && !is_author(c));
            if others {
                for candidate in &mut candidates {
                    if candidate.eligible() && is_author(candidate) {
                        candidate.block =
                            Some(Block::Author("it wrote the change under review".to_owned()));
                    }
                }
                notes.push(format!(
                    "not cross-vendor: only {} is available, so another of its models reviews",
                    name(author.provider)
                ));
            } else {
                notes.push(format!(
                    "not cross-vendor: only {} is available, and only the author's model can \
                     review",
                    name(author.provider)
                ));
            }
        }
    }

    // A second checker: another model than those already checking, when one can take it.
    if !query.distinct_from.is_empty() {
        let taken = |c: &Candidate| {
            query.distinct_from.iter().any(|other| {
                other.provider == c.model.provider
                    && other
                        .model
                        .as_deref()
                        .is_some_and(|model| is_model(&c.model, query.registry, model))
            })
        };
        let is_author = |c: &Candidate| {
            query.avoid.as_ref().is_some_and(|author| {
                author.provider == c.model.provider
                    && author
                        .model
                        .as_deref()
                        .is_some_and(|model| is_model(&c.model, query.registry, model))
            })
        };
        if !candidates.iter().any(|c| c.eligible() && !taken(c)) {
            // Only the models already checking are left of the other vendor: a model of the
            // author's vendor (not the author's own) is more independent than a repeat.
            for candidate in &mut candidates {
                if matches!(candidate.block, Some(Block::Author(_)))
                    && !taken(candidate)
                    && !is_author(candidate)
                {
                    candidate.block = None;
                    cross_vendor = Some(false);
                }
            }
        }
        if candidates.iter().any(|c| c.eligible() && !taken(c)) {
            for candidate in &mut candidates {
                if candidate.eligible() && taken(candidate) {
                    candidate.block =
                        Some(Block::Author("it already checks this change".to_owned()));
                }
            }
        } else {
            notes.push("the same model checks it twice: no other model could take it".to_owned());
        }
    }

    Assessed {
        rules,
        only,
        ranking,
        held,
        candidates,
        notes,
        cross_vendor,
    }
}

/// Routes one task as [`decide`] does, and returns every model weighed with it: the order
/// routing would try them in, and the manual ranking in force place by place.
pub fn preview(query: &Query) -> Preview {
    let Assessed {
        rules,
        only,
        ranking,
        held,
        candidates,
        mut notes,
        cross_vendor,
    } = assessed(query);

    // Order: score, then the table's vendor order, then each CLI's own order.
    let order = table::row(query.category).order;
    let rank = |c: &Candidate| {
        order
            .iter()
            .position(|p| *p == c.model.provider)
            .unwrap_or(2)
    };
    let mut ranked: Vec<usize> = (0..candidates.len())
        .filter(|i| candidates[*i].eligible())
        .collect();
    ranked.sort_by(|a, b| {
        let (a, b) = (&candidates[*a], &candidates[*b]);
        b.score
            .partial_cmp(&a.score)
            .filter(|_| (a.score - b.score).abs() > TIE)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(rank(a).cmp(&rank(b)))
    });
    let manual = ranking.map(|ranking| manual_pick(query, ranking, &candidates, &ranked));
    let places = manual
        .as_ref()
        .map(|manual| manual.places.clone())
        .unwrap_or_default();

    if ranked.is_empty() {
        let waiting = waiting(query, &candidates, &rules, &only, held, ranking);
        return Preview {
            decision: Decision::Wait(waiting),
            candidates: listing(query, &candidates, &ranked, None, ranking),
            places,
        };
    }

    let mut chosen = ranked[0];
    let mut why: Option<String> = None;
    let mut pinned_pick = false;
    let mut rule_text: Option<String> = None;
    // The place whose effort applies.
    let mut place: Option<usize> = None;
    match (ranking, manual.as_ref().and_then(|manual| manual.chosen)) {
        // The ranking decides: its first place that can run, or the model a pin names when the
        // list holds it.
        (Some(ranking), Some((index, entry))) => {
            let text = ranking_text(ranking);
            chosen = index;
            place = Some(entry);
            if let Some(pin) = &query.pin {
                match pinned(query, pin, &candidates, &mut notes) {
                    Some(asked) => match candidates[asked].listed {
                        Some(position) => {
                            chosen = asked;
                            place = Some(position as usize - 1);
                            pinned_pick = true;
                            why = Some(format!("as requested, #{position} in {text}"));
                        }
                        None => notes.push(format!(
                            "{} was asked for, but {text} decides",
                            candidates[asked].label()
                        )),
                    },
                    // A vendor asked for doesn't reorder the list: the ranking decides.
                    None if pin.model.is_none() => {
                        if let Some(provider) = pin.provider {
                            notes.push(format!(
                                "{} was asked for, but {text} decides",
                                name(provider)
                            ));
                        }
                    }
                    None => {}
                }
            }
            let position = place.map_or(1, |entry| entry + 1);
            if why.is_none() {
                why = Some(format!("#{position} in {text}"));
            }
            if let Some(skipped) = manual
                .as_ref()
                .and_then(|manual| first_skipped(&manual.places, position as u32))
            {
                notes.push(skipped);
            }
            if let Some(note) = manual
                .as_ref()
                .and_then(|manual| manual.newest_note.clone())
            {
                notes.push(note);
            }
            if candidates[chosen].model.tier < default_floor(query.category) {
                notes.push(format!(
                    "below the {} floor for {}, ranked by you",
                    tier_name(default_floor(query.category)),
                    table::row(query.category).purpose
                ));
            }
        }
        // Scores decide (with a ranking none of whose models can run, after it).
        (ranking, _) => {
            if let Some(ranking) = ranking {
                let skipped = manual
                    .as_ref()
                    .and_then(|manual| first_skipped(&manual.places, u32::MAX));
                notes.push(match skipped {
                    Some(skipped) => format!(
                        "none of the models in {} can take it ({skipped})",
                        ranking_text(ranking)
                    ),
                    None => format!(
                        "none of the models in {} can take it",
                        ranking_text(ranking)
                    ),
                });
            }
            // A pin, then a `prefer` rule, else the top score. Rules beat pins.
            if let Some(pin) = &query.pin {
                if let Some(index) = pinned(query, pin, &candidates, &mut notes) {
                    chosen = index;
                    why = Some("as requested".to_owned());
                    pinned_pick = true;
                } else if (pin.model.is_none() || held.is_some())
                    && let Some(provider) = pin.provider.or(held)
                {
                    match ranked
                        .iter()
                        .copied()
                        .find(|i| candidates[*i].model.provider == provider)
                    {
                        Some(index) => {
                            chosen = index;
                            why = Some(format!("{} as requested", name(provider)));
                            pinned_pick = true;
                        }
                        None => notes.push(format!(
                            "{} was asked for, but {}",
                            name(provider),
                            provider_block(&candidates, provider)
                        )),
                    }
                }
            }
            if let Some(index) = ranked
                .iter()
                .copied()
                .find(|i| candidates[*i].preferred.is_some())
            {
                if why.is_some() && index != chosen {
                    notes.push(format!(
                        "{} was asked for, but your rule prefers {}",
                        candidates[chosen].label(),
                        candidates[index].label()
                    ));
                }
                chosen = index;
                pinned_pick = false;
                rule_text = candidates[index].preferred.clone();
                why = rule_text.as_ref().map(|rule| format!("your rule: {rule}"));
            } else if !only.is_empty() {
                rule_text = Some(join_rules(&only));
            }
        }
    }
    let manual_pick = place.is_some();

    let pick = &candidates[chosen];
    if why.is_none() && pick.trial {
        let outcomes = pick.model.trial.map_or(0, |trial| trial.outcomes);
        why = Some(format!(
            "a trial of a new model on low-risk work ({outcomes} of {} runs so far)",
            crate::merge::TRIAL_OUTCOMES
        ));
    }
    // Balancing: without quota penalties, another provider would have won.
    let unpenalized = ranked
        .iter()
        .copied()
        .max_by(|a, b| {
            let (a, b) = (&candidates[*a], &candidates[*b]);
            (a.score + a.penalty)
                .total_cmp(&(b.score + b.penalty))
                .then(rank(b).cmp(&rank(a)))
        })
        .unwrap_or(chosen);
    // (Only when the score chose: a pin, a rule or a ranking is not balancing.)
    let balancing = why.is_none()
        && candidates[unpenalized].model.provider != pick.model.provider
        && candidates[unpenalized].penalty > 0.0;
    // Context worth a clause when there is room: the window balancing avoids, or a limit that
    // kept a better model out.
    // Spreading: without load penalties, another provider would have won.
    let unloaded = ranked
        .iter()
        .copied()
        .max_by(|a, b| {
            let (a, b) = (&candidates[*a], &candidates[*b]);
            (a.score + a.load)
                .total_cmp(&(b.score + b.load))
                .then(rank(b).cmp(&rank(a)))
        })
        .unwrap_or(chosen);
    let context = if manual_pick {
        None
    } else if balancing {
        candidates[unpenalized].penalty_window.clone()
    } else if why.is_none() && candidates[unloaded].model.provider != pick.model.provider {
        candidates[unloaded].load_note.clone()
    } else {
        limited_rival(&candidates, pick, rank)
    };

    let tie = ranked
        .iter()
        .filter(|i| **i != chosen)
        .any(|i| (candidates[*i].score - pick.score).abs() <= TIE);
    let why = why.unwrap_or_else(|| score_why(query, pick, tie, rule_text.as_deref()));

    let ranked_effort = ranking
        .zip(place)
        .and_then(|(ranking, entry)| ranking.entries.get(entry))
        .and_then(|entry| entry.effort.as_deref());
    let effort = effort(query, pick, ranked_effort, &mut notes);
    let explanation = Explanation {
        score: (!manual_pick).then(|| round(pick.score)),
        factors: pick.factors.clone(),
        alternatives: alternatives(&candidates, &ranked, chosen, pinned_pick, manual_pick, rank),
        rule: rule_text,
        trial: pick.trial,
        balancing,
        ranking: ranking.map(|ranking| {
            let position = place.map(|entry| entry as u32 + 1);
            RankingUse {
                text: ranking_text(ranking),
                position,
                places: ranking.entries.len() as u32,
                only: ranking.only,
                skipped: places
                    .iter()
                    .filter(|p| p.why.is_some() && position.is_none_or(|at| p.position < at))
                    .cloned()
                    .collect(),
            }
        }),
    };
    let mut reason = format!(
        "{} {}{} for {}: {why}",
        name(pick.model.provider),
        pick.model.id,
        effort
            .as_deref()
            .map(|effort| format!(" ({effort})"))
            .unwrap_or_default(),
        table::row(query.category).purpose,
    );
    if pick.unchecked {
        notes.push(format!(
            "unchecked: {}'s model list isn't loaded yet",
            name(pick.model.provider)
        ));
    }
    // Notes about the request (a pin, a review staying with its vendor, the effort) always
    // show; the context clause only when it fits.
    for note in notes {
        reason.push_str("; ");
        reason.push_str(&note);
    }
    // Balancing always names the window it avoids (the explanation says so too).
    if let Some(context) = context
        && (balancing || reason.len() + context.len() + 3 <= REASON_BUDGET)
    {
        reason.push_str("; ");
        reason.push_str(&context);
    }
    reason.push('.');

    let listing = listing(query, &candidates, &ranked, Some(chosen), ranking);
    let places = places
        .into_iter()
        .map(|mut found| {
            found.chosen = place.is_some_and(|entry| entry as u32 + 1 == found.position);
            found
        })
        .collect();
    Preview {
        decision: Decision::Run(Routed {
            provider: pick.model.provider,
            model: pick.model.id.clone(),
            effort,
            reason,
            explanation,
            cross_vendor,
            trial: pick.trial,
            tier: pick.model.tier,
        }),
        candidates: listing,
        places,
    }
}

// ----- manual rankings -----------------------------------------------------------------------

/// What a manual ranking comes to: the first place that can run (the candidate and the
/// place's index), and every place as routing found it.
struct ManualPick {
    chosen: Option<(usize, usize)>,
    places: Vec<RankedPlace>,
    /// A family place ran an older model because its newest can't run now.
    newest_note: Option<String>,
}

/// Walks the ranking top-down. A model place takes that model; a family place its newest model
/// that can run (the CLI lists newest first; older models kept beside a newer one last); a
/// vendor place its best-scored model that can run.
fn manual_pick(
    query: &Query,
    ranking: &Ranking,
    candidates: &[Candidate],
    ranked: &[usize],
) -> ManualPick {
    let mut chosen = None;
    let mut places = Vec::new();
    let mut newest_note = None;
    for (index, entry) in ranking.entries.iter().enumerate() {
        let position = index as u32 + 1;
        let mut members: Vec<usize> = (0..candidates.len())
            .filter(|i| targets(&entry.target, &candidates[*i].model, query.registry))
            .collect();
        let pick = match &entry.target {
            OverrideTarget::Vendor { .. } => ranked.iter().copied().find(|i| members.contains(i)),
            OverrideTarget::Family { .. } => {
                // Stable: the CLI's own order among the current models, then the older ones.
                members.sort_by_key(|i| candidates[*i].model.legacy);
                members.iter().copied().find(|i| candidates[*i].eligible())
            }
            OverrideTarget::Model { .. } => {
                members.iter().copied().find(|i| candidates[*i].eligible())
            }
        };
        let place = match pick {
            Some(found) => {
                if chosen.is_none() {
                    chosen = Some((found, index));
                    if matches!(entry.target, OverrideTarget::Family { .. })
                        && let Some(newest) = members.first().filter(|newest| **newest != found)
                        && let Some(block) = &candidates[*newest].block
                    {
                        newest_note = Some(format!(
                            "its newest, {}, {}",
                            candidates[*newest].model.id,
                            block.why()
                        ));
                    }
                }
                RankedPlace {
                    position,
                    target: entry.target.clone(),
                    model: Some(candidates[found].model.id.clone()),
                    chosen: false,
                    why: None,
                    resets_at_ms: None,
                }
            }
            None => {
                // The most telling reason among its models: the first one's (the newest, the
                // named one), else why none is listed.
                let first = match &entry.target {
                    OverrideTarget::Vendor { .. } => members
                        .iter()
                        .copied()
                        .filter(|i| !candidates[*i].model.legacy)
                        .max_by(|a, b| {
                            (candidates[*a].score + candidates[*a].penalty)
                                .total_cmp(&(candidates[*b].score + candidates[*b].penalty))
                        })
                        .or_else(|| members.first().copied()),
                    _ => members.first().copied(),
                };
                let (why, resets_at_ms) = match first.and_then(|i| candidates[i].block.as_ref()) {
                    Some(Block::Limit { why, resets_at_ms }) => (why.clone(), *resets_at_ms),
                    Some(block) => (block.why().to_owned(), None),
                    None => (unlisted(query, &entry.target), None),
                };
                RankedPlace {
                    position,
                    target: entry.target.clone(),
                    model: first.map(|i| candidates[i].model.id.clone()),
                    chosen: false,
                    why: Some(why),
                    resets_at_ms,
                }
            }
        };
        places.push(place);
    }
    ManualPick {
        chosen,
        places,
        newest_note,
    }
}

/// Why a ranked place has no model at all: its provider can't be used, or its CLI doesn't list
/// it now.
fn unlisted(query: &Query, target: &OverrideTarget) -> String {
    let provider = target_provider(target);
    let vendor = name(provider);
    match query.providers.iter().find(|s| s.provider == provider) {
        None => format!("{vendor} is not set up"),
        Some(state) if !state.logged_in => format!("{vendor} is not logged in"),
        Some(_) => match target {
            OverrideTarget::Vendor { .. } => format!("{vendor} lists no models now"),
            OverrideTarget::Family { family, .. } => {
                format!("{vendor} lists no {family} model now")
            }
            OverrideTarget::Model { .. } => format!("not in {vendor}'s model list now"),
        },
    }
}

pub(crate) fn target_provider(target: &OverrideTarget) -> ProviderKind {
    match target {
        OverrideTarget::Vendor { provider }
        | OverrideTarget::Family { provider, .. }
        | OverrideTarget::Model { provider, .. } => *provider,
    }
}

/// The first place above `before` that was passed over, and why: "#1 Codex luna: Codex's
/// 5-hour window is used up (resets in 2h 10m)".
fn first_skipped(places: &[RankedPlace], before: u32) -> Option<String> {
    let mut skipped = places
        .iter()
        .filter(|place| place.position < before && place.why.is_some());
    let first = skipped.next()?;
    let more = skipped.count();
    let mut text = format!(
        "#{} {}: {}",
        first.position,
        first.model.as_deref().map_or_else(
            || target_text(&first.target),
            |model| { format!("{} {model}", name(target_provider(&first.target))) }
        ),
        first.why.as_deref().unwrap_or_default()
    );
    if more > 0 {
        text.push_str(&format!(
            " (and {more} more place{} passed over)",
            if more == 1 { "" } else { "s" }
        ));
    }
    Some(text)
}

/// Every model weighed, for the Routing page: the chosen one, the others that can run in the
/// order routing would try them (a manual ranking's places first), then those that can't, by
/// what they would score. An older model the CLI keeps beside a newer one shows only when it
/// can run or is ranked.
fn listing(
    query: &Query,
    candidates: &[Candidate],
    ranked: &[usize],
    chosen: Option<usize>,
    ranking: Option<&Ranking>,
) -> Vec<RouteCandidate> {
    let mut order: Vec<usize> = Vec::new();
    order.extend(chosen);
    let mut runnable: Vec<usize> = ranked
        .iter()
        .copied()
        .filter(|i| Some(*i) != chosen)
        .collect();
    if ranking.is_some() {
        // Stable: places in list order, the rest by score.
        runnable.sort_by_key(|i| candidates[*i].listed.unwrap_or(u32::MAX));
    }
    order.extend(runnable);
    let mut blocked: Vec<usize> = (0..candidates.len())
        .filter(|i| !candidates[*i].eligible())
        .filter(|i| !candidates[*i].model.legacy || candidates[*i].listed.is_some())
        .collect();
    blocked.sort_by(|a, b| {
        let (a, b) = (&candidates[*a], &candidates[*b]);
        a.listed
            .unwrap_or(u32::MAX)
            .cmp(&b.listed.unwrap_or(u32::MAX))
            .then((b.score + b.penalty).total_cmp(&(a.score + a.penalty)))
    });
    order.extend(blocked);
    order
        .into_iter()
        .map(|i| {
            let c = &candidates[i];
            let ranked_effort = ranking
                .zip(c.listed)
                .and_then(|(ranking, position)| ranking.entries.get(position as usize - 1))
                .and_then(|entry| entry.effort.as_deref());
            let effort = c
                .eligible()
                .then(|| effort(query, c, ranked_effort, &mut Vec::new()))
                .flatten();
            let on_trial = matches!(
                c.model.status,
                ModelStatus::Unknown | ModelStatus::Researched
            ) && c
                .model
                .trial
                .is_some_and(|trial| trial.outcomes < crate::merge::TRIAL_OUTCOMES);
            RouteCandidate {
                provider: c.model.provider,
                model: c.model.id.clone(),
                tier: c.model.tier,
                score: c.eligible().then(|| round(c.score)),
                factors: c.factors.clone(),
                effort,
                heat: heat_of(query, &c.model),
                listed: c.listed,
                chosen: Some(i) == chosen,
                trial: c.trial,
                new: on_trial,
                blocked: c.block.as_ref().map(|block| block.why().to_owned()),
                resets_at_ms: match &c.block {
                    Some(Block::Limit { resets_at_ms, .. }) => *resets_at_ms,
                    _ => None,
                },
            }
        })
        .collect()
}

/// The hottest usage window that limits a model (a provider-wide limit counts as Limited).
fn heat_of(query: &Query, model: &MergedModel) -> Option<Heat> {
    let quota = query
        .providers
        .iter()
        .find(|s| s.provider == model.provider)?
        .quota
        .as_ref()?;
    if active_limit(quota.limit.as_ref(), query.now_ms).is_some() {
        return Some(Heat::Limited);
    }
    quota
        .windows
        .iter()
        .filter(|w| window_applies(&w.window, model, query.registry))
        .filter(|w| !has_reset(w.window.resets_at_ms, query.now_ms))
        .map(|w| w.heat)
        .max()
        .or(Some(Heat::Cool))
}

/// Every model routing may consider: the merged catalog (Fable and duplicate aliases of one
/// curated model left out), plus the registry's models for a logged-in provider whose list
/// isn't loaded yet.
fn candidates<'q>(query: &'q Query) -> Vec<Candidate<'q>> {
    let mut models: Vec<(Cow<'q, MergedModel>, bool)> = Vec::new();
    for model in query.models {
        if model.excluded
            || table::names_fable(&model.id)
            || table::names_fable(&model.display_name)
        {
            continue;
        }
        // Claude lists `default` and `opus` for one model: keep the named alias.
        if model.status == ModelStatus::Curated
            && let Some(key) = &model.registry_key
            && let Some(position) = models.iter().position(|(other, _)| {
                other.provider == model.provider
                    && other.status == ModelStatus::Curated
                    && other.registry_key.as_ref() == Some(key)
            })
        {
            if models[position].0.id.eq_ignore_ascii_case("default") {
                models[position] = (Cow::Borrowed(model), false);
            }
            continue;
        }
        models.push((Cow::Borrowed(model), false));
    }
    for state in query.providers {
        let listed = query.models.iter().any(|m| m.provider == state.provider);
        if state.logged_in && !listed {
            for entry in query.registry.entries(state.provider) {
                if let Some(model) = from_entry(entry, state.provider) {
                    models.push((Cow::Owned(model), true));
                }
            }
        }
    }
    models
        .into_iter()
        .map(|(model, unchecked)| Candidate {
            model,
            unchecked,
            trial: false,
            block: None,
            base: 0.0,
            factors: Vec::new(),
            score: 0.0,
            penalty: 0.0,
            penalty_window: None,
            load: 0.0,
            load_note: None,
            learned: None,
            preferred: None,
            listed: None,
        })
        .collect()
}

/// Checks one candidate and scores it.
fn assess<'q>(
    candidate: &mut Candidate<'q>,
    query: &'q Query,
    rules: &[&OverrideRule],
    only: &[&OverrideRule],
    held: Option<ProviderKind>,
    ranking: Option<&Ranking>,
) {
    let model = candidate.model.clone();
    let vendor = name(model.provider);
    let purpose = table::row(query.category).purpose;

    // A new model (unknown or researched) runs only as a trial until it has enough outcomes.
    let on_trial = matches!(model.status, ModelStatus::Unknown | ModelStatus::Researched)
        && model
            .trial
            .is_some_and(|trial| trial.outcomes < crate::merge::TRIAL_OUTCOMES);
    // A model the user ranked runs as ranked: trials only reorder what routing scores.
    candidate.trial =
        on_trial && query.trial_slot && allows_trials(query.category) && candidate.listed.is_none();

    let block = if let Some(exclusion) = query.exclude.iter().find(|ex| {
        ex.provider == model.provider
            && ex
                .model
                .as_deref()
                .is_none_or(|id| is_model(&model, query.registry, id))
    }) {
        Some(Block::Excluded(match exclusion.model {
            Some(_) => "it just failed on this task".to_owned(),
            None => format!("{vendor} just failed on this task"),
        }))
    } else if let Some(rule) = rules.iter().find(|rule| {
        rule.effect == OverrideEffect::Never && targets(&rule.target, &model, query.registry)
    }) {
        Some(Block::Rule(format!("your rule: {}", rule_text(rule))))
    } else if let Some(rule) = only
        .iter()
        .find(|rule| !targets(&rule.target, &model, query.registry))
    {
        // Every applicable `only` rule must allow it.
        Some(Block::Rule(format!("your rule: {}", rule_text(rule))))
    } else if let Some(why) = unmet_need(&model, query.needs) {
        Some(Block::Needs(why))
    } else if let Some(held) = held.filter(|held| *held != model.provider) {
        Some(Block::Pinned(format!("{} was asked for", name(held))))
    } else if let Some(ranking) = ranking.filter(|r| r.only && candidate.listed.is_none()) {
        Some(Block::Rule(format!(
            "not in {} (only these models)",
            ranking_text(ranking)
        )))
    } else if on_trial && !candidate.trial && candidate.listed.is_none() {
        let runs = model.trial.map_or(0, |trial| trial.outcomes);
        Some(Block::Floor(format!(
            "a new model on trial: it runs only on scouting, research and checks given a trial \
             slot ({runs} of {} runs so far)",
            crate::merge::TRIAL_OUTCOMES
        )))
    } else if candidate.listed.is_some() {
        // The user ranked it: the category's own floor is waived, a floor raised for this task
        // holds.
        if query.floor > default_floor(query.category) && model.tier < query.floor {
            Some(Block::Floor(format!(
                "below the {} quality floor asked for this task",
                tier_name(query.floor)
            )))
        } else {
            availability(query.providers, query.registry, query.now_ms, &model)
        }
    } else if model.tier < query.floor && !candidate.trial {
        Some(Block::Floor(if model.tier == QualityTier::Unrated {
            "not rated yet (new models get trial runs on scouting, research and checks)".to_owned()
        } else {
            format!(
                "below the {} quality floor for {purpose}",
                tier_name(query.floor)
            )
        }))
    } else {
        availability(query.providers, query.registry, query.now_ms, &model)
    };
    candidate.block = block;

    // The score, for the eligible and for the explanation of those that are not.
    let strength = model
        .strengths
        .get(&query.category)
        .copied()
        .unwrap_or(crate::merge::UNRATED_STRENGTH);
    candidate.base = strength;
    let mut factors = vec![Factor {
        label: match model.status {
            ModelStatus::Curated => format!("registry strength for {purpose}"),
            ModelStatus::Inherited => format!(
                "{} family strength for {purpose} (inherited)",
                model.family.as_deref().unwrap_or("its")
            ),
            ModelStatus::Researched => format!("research estimate for {purpose}"),
            ModelStatus::Unknown => "not rated yet (default strength)".to_owned(),
        },
        delta: strength,
    }];
    if !query.areas.is_empty() {
        let modifier = query
            .areas
            .iter()
            .map(|area| model.area_strengths.get(area).copied().unwrap_or(0.0))
            .sum::<f64>()
            / query.areas.len() as f64;
        if modifier != 0.0 {
            factors.push(Factor {
                label: format!("strength for {}", areas_text(query.areas)),
                delta: modifier,
            });
        }
    }
    candidate.learned = learned_for(query.learned, &model, query.registry, query.category);
    if let Some(learned) = candidate.learned
        && learned.adjustment != 0.0
    {
        factors.push(Factor {
            label: evidence(learned),
            delta: learned.adjustment,
        });
    }
    if model.status == ModelStatus::Inherited
        && candidate
            .learned
            .is_none_or(|l| l.samples < INHERITED_PROVEN)
    {
        factors.push(Factor {
            label: "a newer model of its family, not yet curated".to_owned(),
            delta: -INHERITED_PENALTY,
        });
    }
    if model.legacy {
        factors.push(Factor {
            label: "an older model (its CLI lists a newer one)".to_owned(),
            delta: -LEGACY_PENALTY,
        });
    }
    if let Some((penalty, window)) = penalty(query, &model) {
        candidate.penalty = penalty;
        candidate.penalty_window = Some(window.clone());
        factors.push(Factor {
            label: window,
            delta: -penalty,
        });
    }
    let running = query
        .running
        .iter()
        .find(|(provider, _)| *provider == model.provider)
        .map_or(0, |(_, count)| *count);
    if running > 0 {
        let label = format!(
            "{vendor} already runs {running} worker{}",
            if running == 1 { "" } else { "s" }
        );
        candidate.load = (f64::from(running) * LOAD_PENALTY).min(MAX_LOAD_PENALTY);
        candidate.load_note = Some(label.clone());
        factors.push(Factor {
            label,
            delta: -candidate.load,
        });
    }
    if candidate.trial {
        factors.push(Factor {
            label: "trial of a new model".to_owned(),
            delta: TRIAL_BONUS,
        });
    }
    candidate.score = factors.iter().map(|factor| factor.delta).sum();
    for factor in &mut factors {
        factor.delta = round(factor.delta);
    }
    candidate.factors = factors;
    candidate.preferred = rules
        .iter()
        .find(|rule| {
            rule.effect == OverrideEffect::Prefer && targets(&rule.target, &model, query.registry)
        })
        .map(|rule| rule_text(rule));
}

fn unmet_need(model: &MergedModel, needs: Needs) -> Option<String> {
    if needs.image_input && !model.modalities.input.contains(&Modality::Image) {
        return Some("doesn't take images".to_owned());
    }
    if needs.image_generation
        && !model
            .modalities
            .tools
            .contains(&Capability::ImageGeneration)
    {
        return Some("can't generate images".to_owned());
    }
    if let Some(need) = needs.context_tokens {
        let window = model.context_window.unwrap_or(ASSUMED_CONTEXT_WINDOW);
        if window < need {
            return Some(format!(
                "its context window ({}) is smaller than the {} tokens needed",
                tokens_text(window),
                tokens_text(need)
            ));
        }
    }
    None
}

/// Whether `model` can run now as far as login and quota go: its provider is logged in, no
/// limit of the provider is in force, and no window that limits it is used up (its own
/// weekly window included). The check [`decide`] makes before scoring.
pub fn available(
    model: &MergedModel,
    providers: &[ProviderState],
    registry: &Registry,
    now_ms: i64,
) -> bool {
    availability(providers, registry, now_ms, model).is_none()
}

/// Login and confirmed limits: a limit is a hard exclusion, never a penalty.
fn availability(
    providers: &[ProviderState],
    registry: &Registry,
    now_ms: i64,
    model: &MergedModel,
) -> Option<Block> {
    let vendor = name(model.provider);
    let Some(state) = providers.iter().find(|s| s.provider == model.provider) else {
        return Some(Block::Unavailable(format!("{vendor} is not set up")));
    };
    if !state.logged_in {
        return Some(Block::Unavailable(format!("{vendor} is not logged in")));
    }
    let quota = state.quota.as_ref()?;
    if let Some(limit) = active_limit(quota.limit.as_ref(), now_ms) {
        let resets = limit.resets_at_ms.or_else(|| {
            let id = limit.window.as_deref()?;
            let window = quota.windows.iter().find(|w| w.window.id == id)?;
            window.window.resets_at_ms
        });
        let why = match limit.kind {
            LimitKind::SpendControl => {
                format!("{vendor}'s spend control stopped it (until a fresh read shows it clear)")
            }
            LimitKind::Credits => format!("{vendor} is out of credits"),
            LimitKind::UsageWindow => {
                let label = limit.window.as_deref().and_then(|id| {
                    quota
                        .windows
                        .iter()
                        .find(|w| w.window.id == id)
                        .map(|w| w.window.label.clone())
                });
                format!(
                    "{} is used up{}",
                    window_name(vendor, label.as_deref()),
                    resets_text(resets, now_ms)
                )
            }
        };
        return Some(Block::Limit {
            why,
            resets_at_ms: if limit.kind == LimitKind::UsageWindow {
                resets
            } else {
                None
            },
        });
    }
    // Every used-up window that limits this model must reset before it runs again.
    let spent: Vec<_> = quota
        .windows
        .iter()
        .filter(|w| window_applies(&w.window, model, registry))
        .filter(|w| !has_reset(w.window.resets_at_ms, now_ms))
        .filter(|w| w.heat == crate::Heat::Limited || w.window.used_percent >= LIMITED_USED)
        .collect();
    let latest = spent.iter().max_by_key(|w| w.window.resets_at_ms)?;
    let resets = if spent.iter().any(|w| w.window.resets_at_ms.is_none()) {
        None
    } else {
        latest.window.resets_at_ms
    };
    let scoped = latest
        .window
        .model
        .as_deref()
        .filter(|scope| {
            !latest
                .window
                .label
                .to_ascii_lowercase()
                .contains(&scope.to_ascii_lowercase())
        })
        .map(|scope| format!(" for {scope}"))
        .unwrap_or_default();
    Some(Block::Limit {
        why: format!(
            "{} is used up{scoped}{}",
            window_name(vendor, Some(&latest.window.label)),
            resets_text(resets, now_ms)
        ),
        resets_at_ms: resets,
    })
}

/// The quota penalty for a model, from the hottest window that limits it.
fn penalty(query: &Query, model: &MergedModel) -> Option<(f64, String)> {
    let state = query
        .providers
        .iter()
        .find(|s| s.provider == model.provider)?;
    let quota = state.quota.as_ref()?;
    quota
        .windows
        .iter()
        .filter(|w| window_applies(&w.window, model, query.registry))
        .filter(|w| !has_reset(w.window.resets_at_ms, query.now_ms))
        .map(|w| {
            let projected = w
                .forecast
                .map_or(w.window.used_percent, |f| f.projected_at_reset);
            (quota_penalty(projected), w, projected)
        })
        .filter(|(penalty, _, _)| *penalty > 0.0)
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(penalty, w, projected)| {
            (
                penalty,
                format!(
                    "{} is projected at {:.0}% by its reset",
                    window_name(name(model.provider), Some(&w.window.label)),
                    projected.min(999.0)
                ),
            )
        })
}

fn has_reset(resets_at_ms: Option<i64>, now_ms: i64) -> bool {
    resets_at_ms.is_some_and(|reset| reset <= now_ms)
}

/// The pinned model, when it may run; otherwise says why not.
fn pinned(
    query: &Query,
    pin: &Pin,
    candidates: &[Candidate],
    notes: &mut Vec<String>,
) -> Option<usize> {
    let wanted = pin.model.as_deref()?.trim();
    if table::names_fable(wanted) {
        notes.push(format!(
            "{wanted} was asked for, but Fable models are never used"
        ));
        return None;
    }
    let found = find_named(query, candidates, pin.provider, wanted);
    match found {
        Some(index) if candidates[index].eligible() => Some(index),
        Some(index) => {
            let why = candidates[index]
                .block
                .as_ref()
                .map_or("it can't take this task", Block::why);
            notes.push(format!("{wanted} was asked for, but {why}"));
            None
        }
        None => {
            notes.push(match pin.provider {
                Some(provider) => {
                    format!(
                        "{wanted} was asked for, but it is not in {}'s model list",
                        name(provider)
                    )
                }
                None => format!("{wanted} was asked for, but no CLI lists it"),
            });
            None
        }
    }
}

/// The vendor a binding pin keeps the task with: the pinned provider, else the vendor of the
/// model it names.
fn held_vendor(query: &Query, candidates: &[Candidate]) -> Option<ProviderKind> {
    if !query.hold_pin {
        return None;
    }
    let pin = query.pin.as_ref()?;
    pin.provider.or_else(|| {
        let wanted = pin.model.as_deref()?.trim();
        find_named(query, candidates, None, wanted).map(|index| candidates[index].model.provider)
    })
}

/// A model asked for by name: its id, a concrete id its entry claims, an id prefix, or a
/// family word.
fn find_named(
    query: &Query,
    candidates: &[Candidate],
    provider: Option<ProviderKind>,
    wanted: &str,
) -> Option<usize> {
    let lower = wanted.to_ascii_lowercase();
    let pool: Vec<usize> = (0..candidates.len())
        .filter(|i| provider.is_none_or(|p| candidates[*i].model.provider == p))
        .collect();
    // At each step, an eligible match before one that can't run (an older `sol` that can,
    // before the newest one at its limit).
    let first = |test: &dyn Fn(&MergedModel) -> bool| {
        let all: Vec<usize> = pool
            .iter()
            .copied()
            .filter(|i| test(&candidates[*i].model))
            .collect();
        all.iter()
            .copied()
            .find(|i| candidates[*i].eligible())
            .or_else(|| all.first().copied())
    };
    first(&|m| m.id.eq_ignore_ascii_case(wanted))
        .or_else(|| first(&|m| is_model(m, query.registry, wanted)))
        .or_else(|| first(&|m| m.id.to_ascii_lowercase().starts_with(&lower)))
        .or_else(|| {
            first(&|m| {
                !wanted.contains(['-', '.'])
                    && (m
                        .family
                        .as_deref()
                        .is_some_and(|f| f.eq_ignore_ascii_case(wanted))
                        || table::has_word(&m.id, wanted))
            })
        })
}

/// Why none of a provider's models can take the task.
fn provider_block(candidates: &[Candidate], provider: ProviderKind) -> String {
    candidates
        .iter()
        .filter(|c| c.model.provider == provider)
        .filter_map(|c| c.block.as_ref())
        .max_by_key(|block| match block {
            Block::Limit { .. } | Block::Unavailable(_) => 3,
            Block::Author(_) | Block::Excluded(_) | Block::Rule(_) | Block::Pinned(_) => 2,
            Block::Floor(_) | Block::Needs(_) => 1,
        })
        .map_or_else(
            || format!("{} has no models listed", name(provider)),
            |block| block.why().to_owned(),
        )
}

/// The effort: the ranked place's, else the pin's, else the registry's for this category, fitted
/// to the model and never above `high`.
fn effort(
    query: &Query,
    pick: &Candidate,
    ranked: Option<&str>,
    notes: &mut Vec<String>,
) -> Option<String> {
    let pinned = query.pin.as_ref().and_then(|pin| pin.effort.as_deref());
    let registry_effort = pick
        .model
        .default_effort
        .get(&query.category)
        .and_then(|effort| clamp_effort(effort))
        .map(|effort| at_least(query.category, effort));
    let wanted = ranked
        .or(pinned)
        .or(registry_effort)
        .or_else(|| category_effort(query.category));
    let (effort, lowered) = table::fit_effort_in(Some(&pick.model.efforts), wanted);
    if lowered && let Some(asked) = ranked.or(pinned) {
        notes.push(format!(
            "effort lowered from {asked} to {}",
            effort.as_deref().unwrap_or("the default")
        ));
    }
    effort
}

// ----- waiting -------------------------------------------------------------------------------

fn waiting(
    query: &Query,
    candidates: &[Candidate],
    rules: &[&OverrideRule],
    only: &[&OverrideRule],
    held: Option<ProviderKind>,
    ranking: Option<&Ranking>,
) -> Waiting {
    let ranking = ranking
        .filter(|ranking| ranking.only)
        .map(|ranking| format!("{} (only these models)", ranking_text(ranking)));
    // Without an `only` rule or ranking, `never` rules that removed models good enough for
    // the task keep it from them: changing those rules lets it run.
    let never: Vec<&OverrideRule> = if only.is_empty() && ranking.is_none() {
        rules
            .iter()
            .copied()
            .filter(|rule| rule.effect == OverrideEffect::Never)
            .filter(|rule| {
                candidates.iter().any(|c| {
                    matches!(c.block, Some(Block::Rule(_)))
                        && c.model.tier >= query.floor
                        && targets(&rule.target, &c.model, query.registry)
                })
            })
            .collect()
    } else {
        Vec::new()
    };
    let rule = if !only.is_empty() {
        Some(join_rules(only))
    } else {
        (!never.is_empty()).then(|| join_rules(&never))
    };
    let (allows, allows_none) = if never.is_empty() {
        ("allows only", "allows no model that")
    } else {
        ("leaves only", "leaves no model that")
    };
    // What keeps the task from other models, in a sentence.
    let holder = match (&rule, &ranking) {
        (Some(rule), Some(ranking)) => Some(format!("your rule ({rule}) with {ranking}")),
        (Some(rule), None) => Some(format!("your rule ({rule})")),
        (None, Some(ranking)) => Some(ranking.clone()),
        (None, None) => None,
    };
    let purpose = table::row(query.category).purpose;
    // Models kept out only by a limit: the task runs when the first of them resets.
    let limited: Vec<(&str, Option<i64>)> = candidates
        .iter()
        .filter_map(|c| match &c.block {
            Some(Block::Limit { why, resets_at_ms }) => Some((why.as_str(), *resets_at_ms)),
            _ => None,
        })
        .collect();
    let earliest = limited
        .iter()
        .min_by_key(|(_, reset)| reset.unwrap_or(i64::MAX))
        .copied();
    let reason = match (earliest, &holder) {
        (Some((why, _)), Some(holder)) => {
            format!("{holder} {allows} models at a limit: {why}")
        }
        (Some((why, _)), None) => why.to_owned(),
        (None, Some(holder)) => {
            format!("{holder} {allows_none} can take this {purpose} work")
        }
        (None, None) => {
            if let Some(held) = held {
                format!(
                    "{} was asked for, but {}",
                    name(held),
                    provider_block(candidates, held)
                )
            } else if !query.providers.iter().any(|s| s.logged_in) {
                "no provider is logged in".to_owned()
            } else {
                format!(
                    "no available model at the {} tier or above can take this {purpose} work",
                    tier_name(query.floor)
                )
            }
        }
    };
    Waiting {
        reason: capitalize(&reason),
        resets_at_ms: earliest.and_then(|(_, reset)| reset),
        rule,
        ranking,
    }
}

// ----- rules ---------------------------------------------------------------------------------

fn applies(rule: &OverrideRule, query: &Query) -> bool {
    rule.project_id
        .as_deref()
        .is_none_or(|project| query.project_id == Some(project))
        && (rule.categories.is_empty() || rule.categories.contains(&query.category))
        && (rule.areas.is_empty() || rule.areas.iter().any(|area| query.areas.contains(area)))
}

/// Whether a rule's target covers a model.
pub fn targets(target: &OverrideTarget, model: &MergedModel, registry: &Registry) -> bool {
    match target {
        OverrideTarget::Vendor { provider } => model.provider == *provider,
        OverrideTarget::Family { provider, family } => {
            model.provider == *provider
                && (model
                    .family
                    .as_deref()
                    .is_some_and(|own| own.eq_ignore_ascii_case(family))
                    || table::has_word(&model.id, family))
        }
        OverrideTarget::Model { provider, id } => {
            model.provider == *provider && is_model(model, registry, id)
        }
    }
}

/// A rule as the user would say it: "never Claude opus for implementation on frontend".
pub fn rule_text(rule: &OverrideRule) -> String {
    let effect = match rule.effect {
        OverrideEffect::Never => "never",
        OverrideEffect::Prefer => "prefer",
        OverrideEffect::Only => "only",
    };
    let target = match &rule.target {
        OverrideTarget::Vendor { provider } => name(*provider).to_owned(),
        OverrideTarget::Family { provider, family } => format!("{} {family}", name(*provider)),
        OverrideTarget::Model { provider, id } => format!("{} {id}", name(*provider)),
    };
    let mut text = format!("{effect} {target}");
    if !rule.categories.is_empty() {
        let purposes: Vec<&str> = rule
            .categories
            .iter()
            .map(|category| table::row(*category).purpose)
            .collect();
        text.push_str(" for ");
        text.push_str(&purposes.join(", "));
    }
    if !rule.areas.is_empty() {
        text.push_str(" on ");
        text.push_str(&areas_text(&rule.areas));
    }
    text
}

fn join_rules(rules: &[&OverrideRule]) -> String {
    rules
        .iter()
        .map(|rule| rule_text(rule))
        .collect::<Vec<_>>()
        .join("; ")
}

// ----- explanation ---------------------------------------------------------------------------

fn score_why(query: &Query, pick: &Candidate, tie: bool, only: Option<&str>) -> String {
    let purpose = table::row(query.category).purpose;
    let mut evidence = if pick.model.status == ModelStatus::Curated {
        format!("registry {}", number(pick.base))
    } else {
        format!("strength {}", number(pick.base))
    };
    if let Some(learned) = pick.learned.filter(|l| l.adjustment.abs() >= 0.3) {
        evidence.push_str("; ");
        evidence.push_str(&evidence_short(learned));
    }
    if tie {
        evidence.push_str(&format!("; ties go to {} here", name(pick.model.provider)));
    }
    let lead = if only.is_some() {
        format!("the best {purpose} score your rule allows")
    } else {
        format!("the top {purpose} score here")
    };
    format!("{lead} ({evidence})")
}

/// Whether `rival`, unpenalized, would have beaten the pick (on score, or on a tie by vendor
/// order).
fn would_beat(rival: &Candidate, pick: &Candidate, rank: &dyn Fn(&Candidate) -> usize) -> bool {
    let (theirs, ours) = (rival.score + rival.penalty, pick.score);
    theirs > ours + TIE || ((theirs - ours).abs() <= TIE && rank(rival) < rank(pick))
}

/// A better model kept out by a limit (or its provider being logged out), if one was.
fn limited_rival(
    candidates: &[Candidate],
    pick: &Candidate,
    rank: impl Fn(&Candidate) -> usize,
) -> Option<String> {
    candidates
        .iter()
        .filter(|c| !c.model.legacy && would_beat(c, pick, &rank))
        .filter(|c| matches!(c.block, Some(Block::Limit { .. } | Block::Unavailable(_))))
        .max_by(|a, b| (a.score + a.penalty).total_cmp(&(b.score + b.penalty)))
        .and_then(|c| c.block.as_ref().map(|block| block.why().to_owned()))
}

/// Up to three models not chosen, most telling first: the runner-up, better models kept out
/// (by a limit, a rule or the floor), the other vendors' best, then the rest.
fn alternatives(
    candidates: &[Candidate],
    ranked: &[usize],
    chosen: usize,
    pinned: bool,
    manual: bool,
    rank: impl Fn(&Candidate) -> usize,
) -> Vec<Alternative> {
    let pick = &candidates[chosen];
    let unpenalized = |c: &&Candidate| c.score + c.penalty;
    let mut order: Vec<&Candidate> = Vec::new();
    let mut runners: Vec<&Candidate> = ranked
        .iter()
        .copied()
        .filter(|i| *i != chosen)
        .map(|i| &candidates[i])
        .collect();
    if manual {
        // The places below the one that ran come next (the skipped ones are in the ranking's
        // own account).
        runners.sort_by_key(|c| c.listed.unwrap_or(u32::MAX));
        order.extend(runners.iter().copied());
    }
    let mut blocked: Vec<&Candidate> = candidates
        .iter()
        .filter(|c| !c.eligible() && !c.model.legacy)
        .collect();
    blocked.sort_by(|a, b| unpenalized(b).total_cmp(&unpenalized(a)));
    order.extend(runners.first());
    order.extend(blocked.iter().filter(|c| would_beat(c, pick, &rank)));
    for provider in ProviderKind::ALL {
        if provider != pick.model.provider {
            order.extend(
                candidates
                    .iter()
                    .filter(|c| c.model.provider == provider && !c.model.legacy)
                    .max_by(|a, b| unpenalized(a).total_cmp(&unpenalized(b))),
            );
        }
    }
    order.extend(runners.iter().copied());
    order.extend(blocked.iter().copied());

    let mut shown: Vec<Alternative> = Vec::new();
    for c in order {
        if shown.len() == 3 {
            break;
        }
        if shown
            .iter()
            .any(|a| a.provider == c.model.provider && a.model == c.model.id)
        {
            continue;
        }
        let why_not = match &c.block {
            Some(block) => block.why().to_owned(),
            None if manual => match c.listed {
                Some(position) => format!("#{position} in your ranking"),
                None => "not in your ranking".to_owned(),
            },
            None if pick.preferred.is_some() && c.preferred.is_none() => {
                "your rule prefers another model".to_owned()
            }
            None if pinned => "not the model asked for".to_owned(),
            None if (c.score - pick.score).abs() <= TIE => {
                "tied, and ties go the other way here".to_owned()
            }
            None => format!(
                "scored lower ({} vs {})",
                number(c.score),
                number(pick.score)
            ),
        };
        shown.push(Alternative {
            provider: c.model.provider,
            model: c.model.id.clone(),
            score: c.eligible().then(|| round(c.score)),
            why_not,
        });
    }
    shown
}

/// Learned evidence for the factor list.
fn evidence(learned: &Learned) -> String {
    format!("learned here: {}", evidence_short(learned))
}

fn evidence_short(learned: &Learned) -> String {
    let n = learned.samples;
    match (learned.review_pass_rate, learned.verification_pass_rate) {
        (Some(rate), _) => format!(
            "{:.0}% of its reviewed work passed first time over {n} task{}",
            rate * 100.0,
            if n == 1 { "" } else { "s" }
        ),
        _ => {
            let good = (learned.success_rate * f64::from(n)).round() as u32;
            format!(
                "{good} of {n} task{} succeeded",
                if n == 1 { "" } else { "s" }
            )
        }
    }
}

fn window_name(vendor: &str, label: Option<&str>) -> String {
    match label {
        Some(label) if !label.is_empty() => {
            let mut chars = label.chars();
            let first = chars
                .next()
                .map(|c| c.to_lowercase().to_string())
                .unwrap_or_default();
            format!("{vendor}'s {first}{} window", chars.as_str())
        }
        _ => format!("{vendor}'s usage window"),
    }
}

/// " (resets in 2h 10m)".
fn resets_text(resets_at_ms: Option<i64>, now_ms: i64) -> String {
    let Some(reset) = resets_at_ms else {
        return String::new();
    };
    let minutes = ((reset - now_ms).max(0) + 59_999) / 60_000;
    let (days, hours, mins) = (minutes / 1440, (minutes % 1440) / 60, minutes % 60);
    let span = if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {mins}m")
    } else {
        format!("{mins}m")
    };
    format!(" (resets in {span})")
}

fn tier_name(tier: QualityTier) -> &'static str {
    match tier {
        QualityTier::Unrated => "unrated",
        QualityTier::Light => "light",
        QualityTier::Standard => "standard",
        QualityTier::Strong => "strong",
        QualityTier::Frontier => "frontier",
    }
}

pub(crate) fn areas_text(areas: &[Area]) -> String {
    areas
        .iter()
        .map(|area| match area {
            Area::Frontend => "frontend",
            Area::Backend => "backend",
            Area::Infra => "infra",
            Area::Docs => "docs",
            Area::Tests => "tests",
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn tokens_text(tokens: i64) -> String {
    if tokens >= 1_000_000 {
        format!("{}M", number(tokens as f64 / 1_000_000.0))
    } else {
        format!("{}k", tokens / 1000)
    }
}

/// A score for text: one decimal, without a trailing `.0`.
fn number(value: f64) -> String {
    let text = format!("{value:.1}");
    text.strip_suffix(".0").map(str::to_owned).unwrap_or(text)
}

fn round(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_verifier_runs_at_medium_effort_at_least() {
        assert_eq!(category_effort(TaskCategory::Verify), Some("medium"));
        assert_eq!(at_least(TaskCategory::Verify, "low"), "medium");
        assert_eq!(at_least(TaskCategory::Verify, "high"), "high");
        assert_eq!(at_least(TaskCategory::Scout, "low"), "low");
    }

    #[test]
    fn code_runs_on_the_best_at_high_and_chores_on_light_at_low() {
        for category in [
            TaskCategory::Implement,
            TaskCategory::Review,
            TaskCategory::Merge,
        ] {
            assert_eq!(default_floor(category), QualityTier::Frontier);
            assert_eq!(category_effort(category), Some("high"));
            assert_eq!(at_least(category, "medium"), "high");
            assert_eq!(at_least(category, "xhigh"), "xhigh");
        }
        assert_eq!(default_floor(TaskCategory::Scout), QualityTier::Light);
        assert_eq!(category_effort(TaskCategory::Scout), Some("low"));
        assert_eq!(at_least(TaskCategory::Scout, "medium"), "low");
    }

    fn info(id: &str, name: &str, resolved: Option<&str>) -> brigadier_providers::ModelInfo {
        brigadier_providers::ModelInfo {
            id: id.to_owned(),
            display_name: name.to_owned(),
            description: String::new(),
            resolved: resolved.map(str::to_owned),
            efforts: vec!["low".into(), "medium".into(), "high".into()],
            default_effort: Some("medium".into()),
            is_default: false,
            input_modalities: vec!["text".into(), "image".into()],
            fast: None,
            legacy: false,
        }
    }

    /// Both CLIs' lists as they are now, Fable included.
    fn catalog(registry: &Registry) -> Vec<MergedModel> {
        let claude = [
            info("opus[1m]", "Opus 5.5", Some("claude-opus-5-5[1m]")),
            info("claude-fable-5-1[1m]", "Fable", Some("claude-fable-5-1")),
            info("sonnet", "Sonnet 5", Some("claude-sonnet-5")),
            info("haiku", "Haiku 4.5", Some("claude-haiku-4-5-20251001")),
            info("claude-opus-5", "Opus 5", None),
        ];
        let codex = [
            info("gpt-6.1-sol", "GPT-6.1-Sol", None),
            info("gpt-6-astra", "GPT-6-Astra", None),
            info("gpt-6-luna", "GPT-6-Luna", None),
            info("gpt-5.6-terra", "GPT-5.6-Terra", None),
        ];
        crate::merge(
            registry,
            &[
                (ProviderKind::Claude, claude.as_slice()),
                (ProviderKind::Codex, codex.as_slice()),
            ],
            &[],
            &[],
        )
    }

    fn rule(effect: OverrideEffect, target: OverrideTarget) -> OverrideRule {
        OverrideRule {
            id: "r".into(),
            effect,
            target,
            categories: Vec::new(),
            areas: Vec::new(),
            project_id: None,
            created_at_ms: 0,
        }
    }

    /// Providers, rules, models that just failed and the author under review.
    type Case<'a> = (
        &'a [ProviderState],
        &'a [OverrideRule],
        &'a [Exclusion],
        Option<Author>,
    );

    #[test]
    fn eligible_is_what_decide_may_pick_and_never_fable() {
        let registry = Registry::bundled();
        let models = catalog(&registry);
        let both = [ProviderKind::Claude, ProviderKind::Codex].map(|provider| ProviderState {
            provider,
            logged_in: true,
            quota: None,
        });
        let claude_only = [
            both[0].clone(),
            ProviderState {
                logged_in: false,
                ..both[1].clone()
            },
        ];
        let never_opus = [rule(
            OverrideEffect::Never,
            OverrideTarget::Family {
                provider: ProviderKind::Claude,
                family: "opus".into(),
            },
        )];
        let only_luna = [rule(
            OverrideEffect::Only,
            OverrideTarget::Model {
                provider: ProviderKind::Codex,
                id: "gpt-6-luna".into(),
            },
        )];
        let failed = [Exclusion {
            provider: ProviderKind::Codex,
            model: None,
        }];
        let author = Author {
            provider: ProviderKind::Claude,
            model: Some("opus[1m]".into()),
        };
        let cases: [Case; 6] = [
            (&both, &[], &[], None),
            (&claude_only, &[], &[], None),
            (&both, &never_opus, &[], None),
            (&both, &only_luna, &[], None),
            (&both, &[], &failed, None),
            (&both, &[], &[], Some(author)),
        ];
        let mut runs = 0;
        for (providers, overrides, exclude, avoid) in cases {
            for category in TaskCategory::ALL {
                let query = Query {
                    category,
                    areas: &[],
                    floor: default_floor(category),
                    needs: Needs::default(),
                    pin: None,
                    hold_pin: false,
                    avoid: avoid.clone(),
                    distinct_from: Vec::new(),
                    exclude,
                    overrides,
                    rankings: &[],
                    project_id: None,
                    running: &[],
                    trial_slot: false,
                    providers,
                    models: &models,
                    registry: &registry,
                    learned: &[],
                    now_ms: 0,
                };
                let eligible = eligible(&query);
                let listed: Vec<(ProviderKind, String)> = eligible
                    .iter()
                    .map(|model| (model.provider, model.id.clone()))
                    .collect();
                for model in &eligible {
                    for name in [Some(&model.id), Some(&model.display_name)]
                        .into_iter()
                        .chain([model.resolved.as_ref()])
                        .flatten()
                    {
                        assert!(!table::names_fable(name), "{category:?}: {name} is Fable");
                    }
                }
                let preview = preview(&query);
                // Exactly the models the preview found able to run.
                let able: Vec<(ProviderKind, String)> = preview
                    .candidates
                    .iter()
                    .filter(|candidate| candidate.blocked.is_none())
                    .map(|candidate| (candidate.provider, candidate.model.clone()))
                    .collect();
                assert_eq!(listed.len(), able.len(), "{category:?}");
                assert!(able.iter().all(|model| listed.contains(model)));
                match decide(&query) {
                    Decision::Run(routed) => {
                        runs += 1;
                        assert!(
                            listed.contains(&(routed.provider, routed.model.clone())),
                            "{category:?}: {} is not eligible",
                            routed.model
                        );
                    }
                    Decision::Wait(_) => assert!(listed.is_empty(), "{category:?}"),
                }
                // A review goes to another vendor than the author's: so do its sub-agents.
                if query.avoid.is_some() && !listed.is_empty() {
                    assert!(
                        listed
                            .iter()
                            .all(|(provider, _)| *provider == ProviderKind::Codex)
                    );
                }
                if overrides == only_luna.as_slice() {
                    assert!(listed.iter().all(|(_, id)| id == "gpt-6-luna"));
                }
                if overrides == never_opus.as_slice() || exclude == failed.as_slice() {
                    assert!(!listed.iter().any(|(provider, id)| {
                        (exclude == failed.as_slice() && *provider == ProviderKind::Codex)
                            || (overrides == never_opus.as_slice() && id.contains("opus"))
                    }));
                }
            }
        }
        assert!(runs > 0);
    }
    fn query<'a>(
        registry: &'a Registry,
        models: &'a [MergedModel],
        providers: &'a [ProviderState],
    ) -> Query<'a> {
        Query {
            category: TaskCategory::Research,
            areas: &[],
            floor: QualityTier::Standard,
            needs: Needs::default(),
            pin: None,
            hold_pin: false,
            avoid: None,
            distinct_from: vec![],
            exclude: &[],
            overrides: &[],
            rankings: &[],
            project_id: None,
            running: &[],
            trial_slot: false,
            providers,
            models,
            registry,
            learned: &[],
            now_ms: 0,
        }
    }

    fn sourced_patch(model: &str) -> crate::RatingPatch {
        serde_json::from_value(serde_json::json!({
            "provider":"codex", "model":model, "sources":["https://example.com/model"],
            "tier":"frontier", "strengths":{"research":10,"implement":10},
            "defaultEffort":{"research":"high"}
        }))
        .unwrap()
    }

    fn routed(query: &Query<'_>) -> Routed {
        match decide(query) {
            Decision::Run(route) => route,
            Decision::Wait(wait) => panic!("unexpected wait: {wait:?}"),
        }
    }

    #[test]
    fn overlay_effort_is_concrete_and_manual_and_pinned_efforts_win() {
        let registry = Registry::bundled();
        let catalog = [
            info("gpt-6-sol", "Sol", None),
            info("sol-alias", "Alias", Some("gpt-6-sol")),
            info("gpt-99-sol", "Future Sol", None),
        ];
        let original = crate::merge(&registry, &[(ProviderKind::Codex, &catalog)], &[], &[]);
        let mut models = original.clone();
        let patch = sourced_patch("gpt-6-sol");
        for model in &mut models {
            patch.apply(model);
        }
        assert_eq!(models[2], original[2]);
        let providers = [ProviderState {
            provider: ProviderKind::Codex,
            logged_in: true,
            quota: None,
        }];
        let mut q = query(&registry, &models[..1], &providers);
        assert_eq!(routed(&q).effort.as_deref(), Some("high"));
        q.models = &models[1..2];
        assert_eq!(routed(&q).effort.as_deref(), Some("high"));
        q.pin = Some(Pin {
            effort: Some("low".into()),
            ..Default::default()
        });
        assert_eq!(routed(&q).effort.as_deref(), Some("low"));
        let rankings = [Ranking {
            id: "manual".into(),
            category: TaskCategory::Research,
            areas: vec![],
            project_id: None,
            manual: true,
            only: false,
            updated_at_ms: 0,
            entries: vec![crate::RankedEntry {
                target: OverrideTarget::Vendor {
                    provider: ProviderKind::Codex,
                },
                effort: Some("medium".into()),
            }],
        }];
        q.rankings = &rankings;
        assert_eq!(routed(&q).effort.as_deref(), Some("medium"));
        q.rankings = &[];
        q.pin = None;
        q.models = &original[..1];
        assert_eq!(
            routed(&q).effort.as_deref(),
            original[0]
                .default_effort
                .get(&TaskCategory::Research)
                .map(String::as_str)
        );
    }

    #[test]
    fn overlay_unknown_still_needs_a_low_risk_trial_slot_and_rules_still_win() {
        let registry = Registry::bundled();
        let catalog = [info("new-model", "New", None)];
        let mut models = crate::merge(&registry, &[(ProviderKind::Codex, &catalog)], &[], &[]);
        sourced_patch("new-model").apply(&mut models[0]);
        let providers = [ProviderState {
            provider: ProviderKind::Codex,
            logged_in: true,
            quota: None,
        }];
        let mut q = query(&registry, &models, &providers);
        assert!(matches!(decide(&q), Decision::Wait(_)));
        q.trial_slot = true;
        assert!(routed(&q).trial);
        q.category = TaskCategory::Implement;
        assert!(matches!(decide(&q), Decision::Wait(_)));
        q.category = TaskCategory::Research;
        let never = [rule(
            OverrideEffect::Never,
            OverrideTarget::Vendor {
                provider: ProviderKind::Codex,
            },
        )];
        q.overrides = &never;
        assert!(matches!(decide(&q), Decision::Wait(_)));
        q.overrides = &[];
        let learned = [Learned {
            provider: ProviderKind::Codex,
            model: "new-model".into(),
            category: TaskCategory::Research,
            samples: 5,
            success_rate: 0.2,
            review_pass_rate: None,
            avg_rework: None,
            verification_pass_rate: None,
            median_duration_ms: None,
            median_tokens: None,
            adjustment: -2.0,
        }];
        let before = preview(&q);
        q.learned = &learned;
        let after = preview(&q);
        assert!(after.candidates[0].score < before.candidates[0].score);
    }

    #[test]
    fn overlay_preserves_alias_dedup_matching_and_inherited_penalty() {
        let registry = Registry::bundled();
        let catalog = [
            info("gpt-6-sol", "Sol", None),
            info("sol-alias", "Alias", Some("gpt-6-sol")),
            info("gpt-99-sol", "Future Sol", None),
        ];
        let mut models = crate::merge(&registry, &[(ProviderKind::Codex, &catalog)], &[], &[]);
        assert_eq!(models[0].status, ModelStatus::Curated);
        assert_eq!(models[2].status, ModelStatus::Inherited);
        for model in &mut models {
            sourced_patch(&model.rating_identity()).apply(model);
        }
        assert!(is_model(&models[1], &registry, "gpt-6-sol"));
        assert!(!is_model(&models[2], &registry, "gpt-6-sol"));
        let providers = [ProviderState {
            provider: ProviderKind::Codex,
            logged_in: true,
            quota: None,
        }];
        let q = query(&registry, &models, &providers);
        let p = preview(&q);
        let able: Vec<_> = p
            .candidates
            .iter()
            .filter(|c| c.blocked.is_none())
            .collect();
        assert_eq!(able.len(), 2);
        let inherited = able.iter().find(|c| c.model == "gpt-99-sol").unwrap();
        let curated = able.iter().find(|c| c.model != "gpt-99-sol").unwrap();
        assert!(inherited.score < curated.score);
    }
}
