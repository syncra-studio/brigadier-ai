//! Sourced, concrete-model rating patches. Facts and matching rules are never patched.
use std::collections::BTreeMap;

use brigadier_providers::ProviderKind;
use serde::{Deserialize, Serialize};

use crate::load::{MAX_AREA_MODIFIER, MAX_STRENGTH};
use crate::{Area, MergedModel, QualityTier, RatingProvenance, TaskCategory};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RatingPatch {
    pub provider: ProviderKind,
    /// Host-captured concrete identity, never a family registry key.
    pub model: String,
    pub sources: Vec<String>,
    pub tier: Option<QualityTier>,
    #[serde(default)]
    pub strengths: BTreeMap<TaskCategory, f64>,
    #[serde(default)]
    pub area_strengths: BTreeMap<Area, f64>,
    #[serde(default)]
    pub default_effort: BTreeMap<TaskCategory, String>,
}

impl RatingPatch {
    pub fn applies(&self, model: &MergedModel) -> bool {
        !model.excluded && self.provider == model.provider && self.model == model.rating_identity()
    }

    pub fn apply(&self, model: &mut MergedModel) {
        if !self.applies(model) {
            return;
        }
        if let Some(tier) = self.tier {
            model.tier = tier;
        }
        model.strengths.extend(self.strengths.clone());
        model.area_strengths.extend(self.area_strengths.clone());
        model.default_effort.extend(
            self.default_effort
                .iter()
                .filter(|(_, effort)| model.efforts.contains(effort))
                .map(|(category, effort)| (*category, effort.clone())),
        );
        model.rating_provenance = RatingProvenance::ResearchedOverlay;
    }

    /// Field paths, including categories, for the refresh summary.
    pub fn changed_fields(&self, model: &MergedModel) -> Vec<String> {
        let mut fields = Vec::new();
        if self.tier.is_some_and(|tier| tier != model.tier) {
            fields.push("tier".into());
        }
        for (category, value) in &self.strengths {
            if model.strengths.get(category) != Some(value) {
                fields.push(format!(
                    "strengths.{}",
                    serde_json::to_value(category).unwrap().as_str().unwrap()
                ));
            }
        }
        for (area, value) in &self.area_strengths {
            if model.area_strengths.get(area) != Some(value) {
                fields.push(format!(
                    "areaStrengths.{}",
                    serde_json::to_value(area).unwrap().as_str().unwrap()
                ));
            }
        }
        for (category, value) in &self.default_effort {
            if model.default_effort.get(category) != Some(value) {
                fields.push(format!(
                    "defaultEffort.{}",
                    serde_json::to_value(category).unwrap().as_str().unwrap()
                ));
            }
        }
        fields
    }
}

/// Drop individual malformed, unsupported, unsourced or out-of-bounds patches.
pub fn validate_patches(reply: &str, models: &[MergedModel]) -> (Vec<RatingPatch>, Vec<String>) {
    let mut errors = Vec::new();
    let result = serde_json::from_str::<serde_json::Value>(reply);
    let values = match result
        .as_ref()
        .ok()
        .and_then(|v| v.get("patches"))
        .and_then(|v| v.as_array())
    {
        Some(values) => values,
        None => {
            return (
                Vec::new(),
                vec!["the research must return a JSON object with a patches array".into()],
            );
        }
    };
    let mut patches: Vec<RatingPatch> = Vec::new();
    for value in values {
        let patch: RatingPatch = match serde_json::from_value(value.clone()) {
            Ok(patch) => patch,
            Err(error) => {
                errors.push(format!("invalid patch: {error}"));
                continue;
            }
        };
        let matching: Vec<_> = models.iter().filter(|model| patch.applies(model)).collect();
        let error = if matching.is_empty() {
            Some("identity is outside the captured catalog")
        } else if !patch.sources.iter().any(|source| {
            source.starts_with("https://")
                && !source.chars().any(char::is_whitespace)
                && url::Url::parse(source)
                    .is_ok_and(|url| url.scheme() == "https" && url.has_host())
        }) {
            Some("no parseable HTTPS source")
        } else if patch.tier == Some(QualityTier::Unrated) {
            Some("unrated is not a researched quality tier")
        } else if patch
            .strengths
            .values()
            .any(|v| !v.is_finite() || !(0.0..=MAX_STRENGTH).contains(v))
            || patch
                .area_strengths
                .values()
                .any(|v| !v.is_finite() || !(-MAX_AREA_MODIFIER..=MAX_AREA_MODIFIER).contains(v))
            || patch.default_effort.values().any(|effort| {
                !matches!(effort.as_str(), "low" | "medium" | "high")
                    || matching.iter().any(|model| !model.efforts.contains(effort))
            })
        {
            Some("rating or effort is outside the model's bounds")
        } else if patch.tier.is_none()
            && patch.strengths.is_empty()
            && patch.area_strengths.is_empty()
            && patch.default_effort.is_empty()
        {
            Some("patch has no rating fields")
        } else if patches
            .iter()
            .any(|prior| prior.provider == patch.provider && prior.model == patch.model)
        {
            Some("duplicate concrete identity")
        } else {
            None
        };
        if let Some(error) = error {
            errors.push(format!("{}: {error}", patch.model));
        } else {
            patches.push(patch);
        }
    }
    if patches.is_empty() {
        errors.push("no valid sourced model ratings; previous overlay kept".into());
    }
    (patches, errors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ModelStatus, Registry, ResearchNote, merge};

    fn info(id: &str, resolved: Option<&str>) -> brigadier_providers::ModelInfo {
        serde_json::from_value(serde_json::json!({
            "id":id, "displayName":id, "description":"", "resolved":resolved,
            "efforts":["low","medium","high"], "defaultEffort":"medium",
            "isDefault":false, "inputModalities":["text"], "legacy":false
        }))
        .unwrap()
    }

    fn models() -> Vec<MergedModel> {
        merge(
            &Registry::bundled(),
            &[(
                ProviderKind::Claude,
                &[
                    info("claude-opus-5-5", None),
                    info("opus", Some("claude-opus-5-5")),
                    info("claude-opus-99", None),
                    info("brand-new", None),
                ],
            )],
            &[],
            &[],
        )
    }

    fn patch(model: &str) -> serde_json::Value {
        serde_json::json!({"provider":"claude", "model":model,
            "sources":["https://example.com/model"], "tier":"frontier",
            "strengths":{"implement":10}, "areaStrengths":{"backend":2},
            "defaultEffort":{"research":"high"}})
    }

    #[test]
    fn validates_each_patch_and_preserves_facts_status_and_trials() {
        let original = models();
        let mut models = original.clone();
        let (patches, errors) = validate_patches(
            &serde_json::json!({"patches":[
                patch("claude-opus-5-5"), patch("brand-new")
            ]})
            .to_string(),
            &models,
        );
        assert!(errors.is_empty(), "{errors:?}");
        for model in &mut models {
            for patch in &patches {
                patch.apply(model);
            }
        }
        assert_eq!(models[0].default_effort[&TaskCategory::Research], "high");
        assert_eq!(models[1].default_effort[&TaskCategory::Research], "high");
        assert_eq!(models[2], original[2]);
        for (before, after) in original.iter().zip(&models) {
            assert_eq!(after.status, before.status);
            assert_eq!(after.registry_key, before.registry_key);
            assert_eq!(after.family, before.family);
            assert_eq!(after.modalities, before.modalities);
            assert_eq!(after.efforts, before.efforts);
            assert_eq!(after.trial, before.trial);
            assert_eq!(after.context_window, before.context_window);
        }
        assert_eq!(models[3].status, ModelStatus::Unknown);
        assert_eq!(models[3].trial.unwrap().outcomes, 0);
        assert_eq!(
            models[3].rating_provenance,
            RatingProvenance::ResearchedOverlay
        );
        let mut moved = original[1].clone();
        moved.resolved = Some("claude-opus-99".into());
        assert!(!patches[0].applies(&moved));
        // The exact id remains valid even after the alias moves.
        assert!(patches[0].applies(&original[0]));
    }

    #[test]
    fn rejects_unrated_tier_but_accepts_rated_and_omitted_tiers() {
        for tier in ["unrated", "light", "standard", "strong", "frontier"] {
            let mut value = patch("brand-new");
            value["tier"] = serde_json::json!(tier);
            let (patches, errors) = validate_patches(
                &serde_json::json!({"patches":[value]}).to_string(),
                &models(),
            );
            if tier == "unrated" {
                assert!(patches.is_empty());
                assert!(errors.iter().any(|error| error.contains("unrated")));
            } else {
                assert_eq!(patches.len(), 1, "{tier}: {errors:?}");
                assert!(errors.is_empty());
            }
        }
        let mut value = patch("brand-new");
        value.as_object_mut().unwrap().remove("tier");
        let (patches, errors) = validate_patches(
            &serde_json::json!({"patches":[value]}).to_string(),
            &models(),
        );
        assert_eq!(patches.len(), 1, "{errors:?}");
        assert!(errors.is_empty());
        assert_eq!(patches[0].tier, None);
    }

    #[test]
    fn drops_invalid_patches_without_poisoning_valid_models() {
        let mut values = vec![patch("claude-opus-5-5")];
        for source in [
            "http://example.com",
            "https://",
            "https://bad host",
            "not a url",
        ] {
            let mut value = patch("brand-new");
            value["sources"] = serde_json::json!([source]);
            values.push(value);
        }
        for (field, value) in [
            ("strengths", serde_json::json!({"implement":11})),
            ("strengths", serde_json::json!({"implement":-1})),
            ("areaStrengths", serde_json::json!({"backend":-3})),
            ("defaultEffort", serde_json::json!({"research":"max"})),
            ("contextWindow", serde_json::json!(100000)),
        ] {
            let mut invalid = patch("brand-new");
            invalid[field] = value;
            values.push(invalid);
        }
        values.push(patch("not-in-catalog"));
        let (patches, errors) = validate_patches(
            &serde_json::json!({"patches":values}).to_string(),
            &models(),
        );
        assert_eq!(patches.len(), 1);
        assert_eq!(errors.len(), 10);
        assert!(
            validate_patches(r#"{"patches":[]}"#, &models())
                .0
                .is_empty()
        );
        assert!(validate_patches("not JSON", &models()).0.is_empty());
        let mut unsupported = models();
        unsupported[3].efforts = vec!["low".into()];
        assert!(
            validate_patches(
                &serde_json::json!({"patches":[patch("brand-new")]}).to_string(),
                &unsupported
            )
            .0
            .is_empty()
        );
    }

    #[test]
    fn sourced_unknown_replaces_bounded_note_but_keeps_trial() {
        let note = ResearchNote {
            provider: ProviderKind::Claude,
            model: "brand-new".into(),
            at_ms: 1,
            summary: "note".into(),
            tier: QualityTier::Frontier,
            strengths: [(TaskCategory::Implement, 10.0)].into(),
            context_window: None,
            knowledge_cutoff: None,
            sources: vec![],
        };
        let mut models = merge(
            &Registry::bundled(),
            &[(ProviderKind::Claude, &[info("brand-new", None)])],
            std::slice::from_ref(&note),
            &[],
        );
        assert_eq!(models[0].tier, QualityTier::Standard);
        assert_eq!(models[0].strengths[&TaskCategory::Implement], 7.0);
        let (patches, _) = validate_patches(
            &serde_json::json!({"patches":[patch("brand-new")]}).to_string(),
            &models,
        );
        patches[0].apply(&mut models[0]);
        assert_eq!(models[0].tier, QualityTier::Frontier);
        assert_eq!(models[0].strengths[&TaskCategory::Implement], 10.0);
        assert_eq!(models[0].status, ModelStatus::Researched);
        assert_eq!(models[0].trial.unwrap().needed, 3);
    }
}
