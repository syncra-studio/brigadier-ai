//! The guard around every driver tool.
//!
//! [`install`] replaces each tool in the driver's registry with a
//! [`GuardedTool`] around it. The registry calls tools only after it has
//! normalized the arguments and authorized the call, and it calls them the
//! same way for a single call, for each `run_steps` step and for its own
//! re-routes and retries. So every rule here holds on every path:
//!
//! 1. never bring an app to the front or move the user's pointer
//!    ([`crate::never_front`]);
//! 2. only tools Brigadier offers ([`crate::catalog`]);
//! 3. only apps the target policy allows ([`crate::target_policy`]), with an
//!    element token resolved to its app first;
//! 4. never an app the user told agents to stop using;
//! 5. a test process runs only tools that need no privacy permission
//!    ([`crate::no_tcc`]). The rules before it never use either, so a test
//!    process goes through them as the real one does.
//!
//! and every result is put in Brigadier's words ([`crate::text`]).

use std::sync::Arc;

use async_trait::async_trait;
use cua_driver_core::protocol::ToolResult;
use cua_driver_core::tool::{ProtectedResourceOwnership, Tool, ToolDef, ToolRegistry};
use serde_json::{json, Value};

use crate::lookup::Lookups;
use crate::target_policy::{self, TargetResolver};
use crate::{catalog, never_front, no_tcc, scope, text, tokens};

/// A driver tool with Brigadier's rules around it.
pub struct GuardedTool {
    inner: Box<dyn Tool>,
    lookups: Arc<dyn Lookups>,
}

impl GuardedTool {
    pub fn new(inner: Box<dyn Tool>, lookups: Arc<dyn Lookups>) -> Self {
        Self { inner, lookups }
    }

    /// The rules, in order. `Err` is the refusal the agent gets; the tool
    /// has not run.
    fn admit(&self, tool: &str, args: &Value) -> Result<(), ToolResult> {
        if let Some(refusal) = never_front::check(tool, args) {
            return Err(refusal);
        }
        if !catalog::offered(tool) {
            return Err(withheld(tool));
        }
        let target_args = resolve_token(tool, args)?;
        let scope = scope::current();
        let lookups = self.lookups.as_ref();
        check_target_with(tool, &target_args, &scope.allowed_bundles, lookups)?;
        if let Some(hook) = scope.on_target.as_ref() {
            let app = crate::lookup::target_app(lookups, tool, &target_args);
            if let Some(message) = hook(tool, app.as_ref()) {
                return Err(stopped(message));
            }
        }
        if let Some(refusal) = no_tcc::refusal(tool) {
            return Err(refusal);
        }
        Ok(())
    }
}

#[async_trait]
impl Tool for GuardedTool {
    fn def(&self) -> &ToolDef {
        self.inner.def()
    }

    fn has_independent_input_lane(&self, args: &Value) -> bool {
        self.inner.has_independent_input_lane(args)
    }

    async fn protected_resource_ownership(
        &self,
        adapter_id: &str,
        args: &Value,
    ) -> ProtectedResourceOwnership {
        self.inner
            .protected_resource_ownership(adapter_id, args)
            .await
    }

    async fn protected_resource_scope(
        &self,
        adapter_id: &str,
        args: &Value,
    ) -> Result<Option<Value>, String> {
        self.inner.protected_resource_scope(adapter_id, args).await
    }

    async fn validate_protected_resource_scope(
        &self,
        adapter_id: &str,
        args: &Value,
        approved_scope: &Value,
    ) -> Result<(), String> {
        self.inner
            .validate_protected_resource_scope(adapter_id, args, approved_scope)
            .await
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let tool = self.inner.def().name.clone();
        if let Err(refusal) = self.admit(&tool, &args) {
            return refusal;
        }
        let result = self.inner.invoke(args).await;
        // Windows and Linux: a refusal the platform can't get past, said plainly.
        #[cfg(not(target_os = "macos"))]
        let result = crate::platform::explain(result);
        tokens::record(&result);
        text::result_for_tool(&tool, result)
    }
}

/// The target policy alone, as [`GuardedTool`] applies it, with the real
/// lookups: for a caller that refuses a call before it reaches the
/// registry, e.g. in another process. An element token is checked by the
/// guard that resolves it, not here.
pub fn check_target(tool: &str, args: &Value, allowed: &[String]) -> Result<(), ToolResult> {
    check_target_with(tool, args, allowed, &crate::lookup::System)
}

fn check_target_with(
    tool: &str,
    args: &Value,
    allowed: &[String],
    lookups: &dyn Lookups,
) -> Result<(), ToolResult> {
    target_policy::enforce(
        tool,
        args,
        allowed,
        &TargetResolver {
            app_for_pid: &|pid| lookups.app_for_pid(pid),
            pid_for_window: &|window| lookups.pid_for_window(window),
        },
    )
}

/// Put every tool of `registry` behind the guard, with the real lookups.
pub fn install(registry: &mut ToolRegistry) {
    install_with(registry, Arc::new(crate::lookup::System));
}

/// [`install`] with the given lookups.
pub fn install_with(registry: &mut ToolRegistry, lookups: Arc<dyn Lookups>) {
    let names: Vec<String> = registry.tool_names().map(str::to_owned).collect();
    for name in names {
        let lookups = lookups.clone();
        registry.wrap(&name, move |inner| {
            Box::new(GuardedTool::new(inner, lookups)) as Box<dyn Tool>
        });
    }
}

/// `args` with the `pid` (and `window_id`) an element token names filled in,
/// so the policy checks the app the token belongs to. A token the guard
/// can't place is refused rather than let through.
fn resolve_token(tool: &str, args: &Value) -> Result<Value, ToolResult> {
    let Some(token) = args.get("element_token").and_then(Value::as_str) else {
        return Ok(args.clone());
    };
    let has = |key: &str| args.get(key).is_some_and(|value| !value.is_null());
    let mut resolved = args.clone();
    match tokens::owner(token) {
        Some((pid, window)) => {
            if let Some(object) = resolved.as_object_mut() {
                if !has("pid") {
                    object.insert("pid".to_owned(), pid.into());
                }
                if let (false, Some(window)) = (has("window_id"), window) {
                    object.insert("window_id".to_owned(), window.into());
                }
            }
            Ok(resolved)
        }
        None if has("pid") => Ok(resolved),
        None => Err(unresolved(tool)),
    }
}

fn withheld(tool: &str) -> ToolResult {
    let message = format!(
        "{tool} is not available in Brigadier: {}.",
        catalog::withheld_reason(tool)
    );
    ToolResult::error(message.clone()).with_structured(json!({
        "status": "refused",
        "code": "tool_not_offered",
        "refusal": { "code": "tool_not_offered", "message": message },
    }))
}

fn unresolved(tool: &str) -> ToolResult {
    let message = format!(
        "{tool} was not done: its element_token is unknown or out of date. Call \
         get_window_state again and use a token from that answer."
    );
    ToolResult::error(message.clone()).with_structured(json!({
        "status": "refused",
        "code": "target_unresolved",
        "refusal": { "code": "target_unresolved", "message": message },
    }))
}

fn stopped(message: String) -> ToolResult {
    ToolResult::error(message.clone()).with_structured(json!({
        "status": "refused",
        "code": "stopped_by_user",
        "error": "stopped_by_user",
        "message": message,
    }))
}
