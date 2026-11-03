//! Automatic picks from the ratings: for a kind of work, each agent's best-rated model, the
//! agents ordered by that rating. Settings › Routing's Automatic rows and `brigadier models`'
//! guidance come from these (the shipped table only stands in where nothing is rated).
//!
//! A model counts for a kind when it is rated (not `unrated`), not excluded, not legacy, takes
//! every category the kind is rated by (its `only`), and scores above 0 there. A kind rated by
//! several categories takes their mean; [`Rating::Overall`] is the mean of every category the
//! model takes except short git text.

use brigadier_providers::ProviderKind;

use crate::{MergedModel, QualityTier, TaskCategory};

/// What a kind of work is rated by.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Rating {
    /// The mean of these categories' strengths.
    Categories(&'static [TaskCategory]),
    /// The strongest overall: the mean of every category the model takes, but short git text.
    Overall,
}

/// How a kind of work picks its models.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PickRule {
    pub rating: Rating,
    /// Only fast and mid-sized models (light and standard tiers); ties go to the lighter one.
    pub small: bool,
    /// The category whose default effort the pick runs at.
    pub effort_from: TaskCategory,
    /// The effort for a model with no default effort for that category, if it takes it.
    pub fallback_effort: Option<&'static str>,
    /// Only models that take images (seeing the screen to use apps on it).
    pub needs_images: bool,
}

/// One agent's pick for a kind of work.
#[derive(Debug, Clone, PartialEq)]
pub struct Pick {
    pub provider: ProviderKind,
    /// The listed model id.
    pub model: String,
    /// The rating's default effort for the kind, if it has one the model takes.
    pub effort: Option<String>,
    /// The kind's rating, 0–10.
    pub score: f64,
    pub tier: QualityTier,
}

/// `model`'s rating for a kind rated by `rating`, when it counts for it (see the module docs).
pub fn score(model: &MergedModel, rating: Rating) -> Option<f64> {
    if model.excluded || model.legacy || model.tier == QualityTier::Unrated {
        return None;
    }
    let takes = |category: &TaskCategory| {
        model
            .only
            .as_ref()
            .is_none_or(|only| only.contains_key(category))
    };
    let categories: Vec<TaskCategory> = match rating {
        Rating::Categories(categories) => {
            if !categories.iter().all(takes) {
                return None;
            }
            categories.to_vec()
        }
        // A model kept for some jobs only is never the strongest overall.
        Rating::Overall if model.only.is_some() => return None,
        Rating::Overall => TaskCategory::ALL
            .into_iter()
            .filter(|category| *category != TaskCategory::GitText)
            .collect(),
    };
    let values: Vec<f64> = categories
        .iter()
        .filter_map(|category| model.strengths.get(category).copied())
        .collect();
    if values.is_empty() || values.len() != categories.len() {
        return None;
    }
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    (mean.is_finite() && mean > 0.0).then_some(mean)
}

/// The tier `model` counts as for `rating`: its own for a kind it is kept for (`only`).
fn tier_for(model: &MergedModel, rating: Rating) -> QualityTier {
    match (rating, &model.only) {
        (Rating::Categories([first, ..]), Some(only)) => {
            only.get(first).copied().unwrap_or(model.tier)
        }
        _ => model.tier,
    }
}

/// Each agent's best model for a kind picked by `rule`, best first (see the module docs).
/// `allowed` leaves out models the user switched off. Equal scores go to the higher tier (the
/// lighter for a small kind), then to the CLI's own order.
pub fn picks(
    models: &[MergedModel],
    rule: PickRule,
    allowed: impl Fn(&MergedModel) -> bool,
) -> Vec<Pick> {
    let mut best: Vec<(Pick, usize)> = Vec::new();
    for (index, model) in models.iter().enumerate() {
        let Some(score) = score(model, rule.rating) else {
            continue;
        };
        let tier = tier_for(model, rule.rating);
        if rule.small && !matches!(model.tier, QualityTier::Light | QualityTier::Standard) {
            continue;
        }
        if rule.needs_images && !model.modalities.input.contains(&crate::Modality::Image) {
            continue;
        }
        if !allowed(model) {
            continue;
        }
        let effort = model
            .default_effort
            .get(&rule.effort_from)
            .filter(|effort| model.efforts.contains(effort))
            .cloned()
            .or_else(|| {
                rule.fallback_effort
                    .filter(|effort| model.efforts.iter().any(|e| e == effort))
                    .map(str::to_owned)
            });
        let pick = Pick {
            provider: model.provider,
            model: model.id.clone(),
            effort,
            score,
            tier,
        };
        match best
            .iter_mut()
            .find(|(held, _)| held.provider == model.provider)
        {
            Some(held) if better(&pick, &held.0, rule.small) => *held = (pick, index),
            Some(_) => {}
            None => best.push((pick, index)),
        }
    }
    best.sort_by(|(a, ai), (b, bi)| {
        if better(a, b, rule.small) {
            std::cmp::Ordering::Less
        } else if better(b, a, rule.small) {
            std::cmp::Ordering::Greater
        } else {
            ai.cmp(bi)
        }
    });
    best.into_iter().map(|(pick, _)| pick).collect()
}

/// Whether `a` beats `b`: a higher score, then the higher tier (the lighter for a small kind).
fn better(a: &Pick, b: &Pick, small: bool) -> bool {
    if (a.score - b.score).abs() > f64::EPSILON {
        return a.score > b.score;
    }
    if small {
        a.tier < b.tier
    } else {
        a.tier > b.tier
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Registry, merge};

    fn info(id: &str) -> brigadier_providers::ModelInfo {
        serde_json::from_value(serde_json::json!({
            "id": id, "displayName": id, "description": "",
            "efforts": ["low", "medium", "high"], "isDefault": false,
            "inputModalities": ["text"], "legacy": false
        }))
        .unwrap()
    }

    /// A small registry: two Claude models, two Codex models, one of them kept for one job.
    fn registry() -> Registry {
        let entry = |key: &str, cli: &str, id: &str, tier: &str, strengths: serde_json::Value| {
            serde_json::json!({
                "key": key, "vendor": if cli == "claude" { "anthropic" } else { "openai" },
                "cli": cli, "match": {"ids": [id]}, "tier": tier,
                "efforts": ["low", "medium", "high"], "strengths": strengths,
                "defaultEffort": {"implement": "medium", "operate": "high", "orchestrate": "high"},
                "released": "2026-01-01", "sources": ["https://example.com"]
            })
        };
        let mut eyes = entry(
            "eyes",
            "codex",
            "eyes",
            "light",
            serde_json::json!({"operate": 10}),
        );
        eyes["only"] = serde_json::json!({"operate": "strong"});
        let document = serde_json::json!({
            "schemaVersion": 1, "revision": 999_999, "updated": "2026-11-01",
            "models": [
                entry("big", "claude", "big", "frontier",
                    serde_json::json!({"implement": 8, "operate": 9, "orchestrate": 9, "design": 8, "gitText": 3})),
                entry("small", "claude", "small", "light",
                    serde_json::json!({"implement": 6, "scout": 8, "operate": 6, "gitText": 7})),
                entry("sol", "codex", "sol", "strong",
                    serde_json::json!({"implement": 9, "operate": 7, "orchestrate": 8, "design": 9})),
                eyes,
            ]
        });
        Registry::parse(&serde_json::to_vec(&document).unwrap()).unwrap()
    }

    fn models() -> Vec<MergedModel> {
        merge(
            &registry(),
            &[
                (
                    ProviderKind::Claude,
                    &[info("big"), info("small"), info("brand-new")],
                ),
                (ProviderKind::Codex, &[info("sol"), info("eyes")]),
            ],
            &[],
            &[],
        )
    }

    fn rule(rating: Rating, small: bool, effort_from: TaskCategory) -> PickRule {
        PickRule {
            rating,
            small,
            effort_from,
            fallback_effort: Some("low"),
            needs_images: false,
        }
    }

    fn names(picks: &[Pick]) -> Vec<(&str, f64)> {
        picks.iter().map(|p| (p.model.as_str(), p.score)).collect()
    }

    #[test]
    fn picks_are_one_row_per_agent_in_rating_order() {
        let models = models();
        let implement = picks(
            &models,
            rule(
                Rating::Categories(&[TaskCategory::Implement]),
                false,
                TaskCategory::Implement,
            ),
            |_| true,
        );
        assert_eq!(names(&implement), [("sol", 9.0), ("big", 8.0)]);
        assert_eq!(implement[1].effort.as_deref(), Some("medium"));
        // A model kept for one job is picked there, and nowhere else.
        let operate = picks(
            &models,
            rule(
                Rating::Categories(&[TaskCategory::Operate]),
                false,
                TaskCategory::Operate,
            ),
            |_| true,
        );
        assert_eq!(names(&operate), [("eyes", 10.0), ("big", 9.0)]);
        // Two categories: their mean.
        let architecture = picks(
            &models,
            rule(
                Rating::Categories(&[TaskCategory::Orchestrate, TaskCategory::Design]),
                false,
                TaskCategory::Orchestrate,
            ),
            |_| true,
        );
        assert_eq!(names(&architecture), [("big", 8.5), ("sol", 8.5)]);
        // A small kind passes over the frontier and strong models; an unrated model never counts.
        let titles = picks(
            &models,
            rule(
                Rating::Categories(&[TaskCategory::GitText]),
                true,
                TaskCategory::GitText,
            ),
            |_| true,
        );
        assert_eq!(names(&titles), [("small", 7.0)]);
        assert_eq!(titles[0].effort.as_deref(), Some("low"));
        assert!(titles.iter().all(|p| p.model != "brand-new"));
        // A model the user switched off is left out: the agent's next best takes its place.
        let without = picks(
            &models,
            rule(
                Rating::Categories(&[TaskCategory::Operate]),
                false,
                TaskCategory::Operate,
            ),
            |model| model.id != "eyes",
        );
        assert_eq!(names(&without), [("big", 9.0), ("sol", 7.0)]);
        let overall = picks(
            &models,
            rule(Rating::Overall, false, TaskCategory::Orchestrate),
            |_| true,
        );
        assert_eq!(overall[0].model, "big");
        assert_eq!(overall.len(), 2);
    }
}
