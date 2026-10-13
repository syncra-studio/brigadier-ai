//! The trusted target allow list on its way from a trusted entry point (the
//! stdio MCP proxy, one-shot `call`) to the daemon's registry.
//!
//! The list travels as the private argument [`ALLOWED_TARGET_BUNDLE_IDS_ARG`].
//! Every trusted entry point first removes any model-supplied copy with
//! [`scope_target_args`], so a tool argument can never widen the scope.

use serde_json::{json, Value};

/// Private tool argument carrying the trusted allow list to the registry.
pub const ALLOWED_TARGET_BUNDLE_IDS_ARG: &str = "_cu_allowed_target_bundle_ids";

/// Parse the allow list from its env form: exact ids, lower-cased. Entries
/// with a wildcard are dropped, since the list only names exact bundles.
fn parse_allowed(raw: &str) -> Vec<String> {
    let mut out: Vec<String> = raw
        .split(|c: char| c == ',' || c.is_whitespace())
        .map(|entry| entry.trim().to_ascii_lowercase())
        .filter(|entry| !entry.is_empty() && !entry.contains('*'))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Replace any caller-supplied allow list in `args` with the trusted one.
/// Call at each trusted entry point before the registry sees the args.
pub fn scope_target_args(args: &mut Value, allowed: &[String]) {
    let Some(object) = args.as_object_mut() else {
        return;
    };
    object.remove(ALLOWED_TARGET_BUNDLE_IDS_ARG);
    if !allowed.is_empty() {
        object.insert(ALLOWED_TARGET_BUNDLE_IDS_ARG.to_owned(), json!(allowed));
    }
}

/// Remove the trusted allow list from `args` and return it, so tools never
/// see the private argument.
pub fn take_allowed(args: &mut Value) -> Vec<String> {
    let Some(value) = args
        .as_object_mut()
        .and_then(|object| object.remove(ALLOWED_TARGET_BUNDLE_IDS_ARG))
    else {
        return Vec::new();
    };
    let joined = value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(",")
        })
        .unwrap_or_default();
    parse_allowed(&joined)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_model_supplied_scope_is_replaced_by_the_trusted_one() {
        let mut args =
            json!({"pid": 101, ALLOWED_TARGET_BUNDLE_IDS_ARG: ["ai.brigadier.dev.agt1"]});
        scope_target_args(&mut args, &[]);
        assert!(args.get(ALLOWED_TARGET_BUNDLE_IDS_ARG).is_none());
        assert!(take_allowed(&mut args).is_empty());

        let mut args = json!({"pid": 101, ALLOWED_TARGET_BUNDLE_IDS_ARG: ["com.apple.terminal"]});
        scope_target_args(&mut args, &["ai.brigadier.dev.agt1".to_owned()]);
        assert_eq!(
            take_allowed(&mut args),
            vec!["ai.brigadier.dev.agt1".to_owned()]
        );
        assert!(args.get(ALLOWED_TARGET_BUNDLE_IDS_ARG).is_none());
    }

    #[test]
    fn the_env_form_takes_exact_ids_only() {
        assert_eq!(
            parse_allowed(" ai.brigadier.dev.A, ai.brigadier.app.* \ncom.apple.Terminal"),
            vec![
                "ai.brigadier.dev.a".to_owned(),
                "com.apple.terminal".to_owned()
            ]
        );
    }
}
