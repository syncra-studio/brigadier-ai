//! The model's own maximum window, from Codex's catalog. `model/list` has no window fields;
//! its refresh writes `models_cache.json` in the account's home. A configured catalog takes
//! precedence. No model names or token ceilings are guessed here (S26).

use std::path::Path;

use serde_json::{Map, Value, json};

use super::{Rpc, p};
use crate::{Error, Result};

pub(super) async fn configure(
    rpc: &Rpc,
    model: Option<&str>,
    effective: &Value,
    home: Option<&Path>,
    cwd: &Path,
    config: &mut Map<String, Value>,
) -> Result<()> {
    // Also refreshes the account's catalog on a fresh install, before reading its metadata.
    let models: p::ModelListResponse = rpc
        .call("model/list", &p::ModelListParams::default())
        .await?;
    let model = model
        .or_else(|| config.get("model").and_then(Value::as_str))
        .or_else(|| effective.pointer("/config/model").and_then(Value::as_str))
        .or_else(|| {
            models
                .data
                .iter()
                .find(|m| m.is_default)
                .map(|m| m.model.as_str())
        })
        .ok_or_else(|| Error::Invalid("Codex did not report its default model".into()))?;
    let path = match effective
        .pointer("/config/model_catalog_json")
        .and_then(Value::as_str)
    {
        Some(path) => cwd.join(path),
        None => home
            .ok_or_else(|| Error::Invalid("Codex's home is unavailable".into()))?
            .join("models_cache.json"),
    };
    let text = tokio::fs::read_to_string(&path).await.map_err(|error| {
        Error::Invalid(format!(
            "Could not read Codex's model catalog {}: {error}",
            path.display()
        ))
    })?;
    let catalog: Value = serde_json::from_str(&text).map_err(|error| {
        Error::Invalid(format!(
            "Could not read Codex's model catalog {}: {error}",
            path.display()
        ))
    })?;
    let settings = settings(&catalog, model)?;
    config.extend(settings);
    Ok(())
}

fn settings(catalog: &Value, model: &str) -> Result<Map<String, Value>> {
    let window = catalog
        .get("models")
        .and_then(Value::as_array)
        .and_then(|models| {
            models
                .iter()
                .find(|entry| entry.get("slug").and_then(Value::as_str) == Some(model))
        })
        .and_then(|entry| entry.get("max_context_window"))
        .and_then(Value::as_i64)
        .filter(|window| *window >= 10)
        .ok_or_else(|| {
            Error::Invalid(format!(
                "Codex's catalog has no max_context_window for {model}"
            ))
        })?;
    // Codex compacts at its own 90% ceiling even if asked for more. Name that limit rather
    // than pretending auto_compact=false disables it; the shared seam runs below it.
    Ok(Map::from_iter([
        ("model_context_window".into(), json!(window)),
        (
            "model_auto_compact_token_limit".into(),
            json!(window / 10 * 9),
        ),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_model_uses_its_own_maximum_instead_of_its_default_window() {
        let catalog = json!({"models": [
            {"slug": "large", "context_window": 500000, "max_context_window": 872000},
            {"slug": "small", "context_window": 128000, "max_context_window": 272000}
        ]});
        for (model, window, compact) in [("large", 872000, 784800), ("small", 272000, 244800)] {
            let config = settings(&catalog, model).unwrap();
            assert_eq!(config["model_context_window"], window);
            assert_eq!(config["model_auto_compact_token_limit"], compact);
            assert_eq!(config.len(), 2);
        }
    }

    #[test]
    fn absent_or_invalid_maxima_are_reported_instead_of_guessed() {
        for metadata in [
            json!({}),
            json!({"max_context_window": null}),
            json!({"max_context_window": 0}),
            json!({"max_context_window": -1}),
        ] {
            let mut entry = metadata;
            entry["slug"] = json!("selected");
            let catalog = json!({"models": [entry]});
            assert!(settings(&catalog, "selected").is_err());
            assert!(settings(&catalog, "missing").is_err());
        }
    }
}
