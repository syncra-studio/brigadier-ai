//! Brigadier's outward MCP view. No registry, platform or daemon policy changes.
//!
//! Keep the original schemas for dispatch validation. Only the list sent over
//! stdio is shortened. Successful Claude Code and Codex calls both expose
//! structuredContent and images; Claude Code errors expose text only. See the
//! live-client evidence recorded with this change. Error text always survives.

#[cfg(test)]
use serde_json::json;
use serde_json::Value;
use std::collections::HashMap;

#[path = "read_quality.rs"]
mod read_quality;

/// Remember the full view only for diffs which report an actual change.
#[derive(Default)]
pub struct View {
    windows: HashMap<(i64, i64), read_quality::Quality>,
}

const ROSTER: &[&str] = &[
    "get_window_state",
    "list_windows",
    "list_apps",
    "page",
    "zoom",
    "click",
    "double_click",
    "right_click",
    "type_text",
    "set_value",
    "press_key",
    "hotkey",
    "scroll",
    "invoke_menu",
    "run_steps",
    "launch_app",
    "kill_app",
    "verify_state",
    "start_recording",
    "stop_recording",
];

pub fn tools_list(mut list: Value, brigadier: bool) -> Value {
    if !brigadier {
        return list;
    }
    if let Some(tools) = list.get_mut("tools").and_then(Value::as_array_mut) {
        tools.retain(|tool| {
            tool["name"]
                .as_str()
                .is_some_and(|name| ROSTER.contains(&name))
        });
        for tool in tools {
            let name = tool["name"].as_str().unwrap_or_default().to_owned();
            tool["description"] = description(&name).into();
            if let Some(object) = tool.as_object_mut() {
                object.remove("outputSchema");
                // Driver inventory extensions are not part of MCP's Tool
                // contract. Safety annotations and every input constraint stay.
                object.remove("capabilities");
                object.remove("risk");
            }
            shorten_schema(&mut tool["inputSchema"]);
            if name == "get_window_state" {
                for (field, description) in [
                    ("include_screenshot", "Default false; thin trees automatically include an image. Explicit false disables fallback; true requests an image."),
                    ("timeout_ms", "AX walk budget in ms; returns a partial tree on timeout. Increase or narrow query/limits."),
                ] {
                    if let Some(property) = tool["inputSchema"]["properties"].get_mut(field) {
                        property["description"] = description.into();
                    }
                }
            }
        }
    }
    list
}

fn description(name: &str) -> &'static str {
    match name {
        "get_window_state" => "Read a window's accessibility tree; screenshots only on request or a thin tree. Row [N] with snapshot_id S is element_token S:N. A new read stales the previous tokens. Re-read with since:\"latest\" for a diff and fresh ids; obey reindexed mappings. Use a screenshot before zoom or pixel actions. Partial trees say truncated; narrow query or increase limits.",
        "list_windows" => "List windows, titles and ids. Prefer an app or pid filter. Use the returned pid/window_id for reads and actions.",
        "list_apps" => "List running and installed apps, one short entry each. To open a known app, call launch_app directly.",
        "page" => "Browser text, DOM and JavaScript. Prefer get_text/query_dom to large trees. CDP may need launch_app with cdp_debugging_port; select a unique tab with target_url_contains. Safari uses Apple Events (enabling requires user confirmation and restarts the browser). Focus the DOM field before inserting text. AX fallback has fewer DOM capabilities.",
        "zoom" => "Crop the last screenshot; take get_window_state with include_screenshot:true first. Region uses that image's pixels. Omit pid to resolve the unique session/window snapshot. Click in the zoom image with from_zoom:true.",
        "click" => "Click a fresh element_token (preferred) or x/y in the target window's last screenshot. Default AX action press. Unsupported actions are refused; listed AX actions are hints, not proof of delivery. Read again to verify the outcome.",
        "double_click" => "Double-click a fresh element_token or screenshot x/y in a specific window. Verify with a read.",
        "right_click" => "Open a context menu with a fresh element_token or window-local screenshot x/y. Verify with a read.",
        "type_text" => "Insert text, preferably into a fresh element_token. Without a token, target the window's focused field; with x/y, click that screenshot point first. Verify the value afterwards.",
        "set_value" => "Set a writable accessibility element's value using a fresh element_token; reads back the result. File names in Finder require the Rename menu workflow instead.",
        "press_key" => "Send a key with optional modifiers to an element or window. With x/y, focus that screenshot point first. Verify the outcome.",
        "hotkey" => "Send a key combination to an element or window; menu equivalents may be used. With x/y, focus that screenshot point first. Verify the outcome.",
        "scroll" => "Scroll a window or element. With x/y, wheel-scroll at that window-local screenshot point. Defaults: amount 3, by line. Read again for fresh ids and outcome.",
        "invoke_menu" => "Invoke a menu by its label path on a specific window, e.g. [\"File\",\"Save\"]. Read to verify the outcome.",
        "run_steps" => "Run ordered actions; stop on the first failure. Steps use {tool,args} or {<tool>:args}. Named targeting: role/name/nth/app/window instead of ids. A step may have wait_for (before), expect (after), timeout_ms, or only checks. Named matches must be unique; nth is zero-based. Prefer short batches. observe:{} performs one final text diff with fresh ids; omit to skip. Delivery alone does not prove the requested outcome.",
        "launch_app" if cfg!(target_os = "linux") => "Open a Linux GUI app in the background. Prefer launch_path from list_apps; bundle_id is ignored on Linux. name may be an executable or installed app name. Returns pid and windows; read the target window next. URLs open through the desktop's URL handler.",
        "launch_app" if cfg!(target_os = "windows") => "Open or reuse a Windows GUI app in the background. Use launch_path from list_apps, path for an executable, or aumid/bundle_id for a packaged app. name searches installed apps. Returns pid and windows; read the target window next. Packaged and elevated apps may refuse background input.",
        "launch_app" => "Open or reuse an app in the background, optionally opening files/URLs. Prefer bundle_id; name may be a full app path. Returns pid and windows; read the target window next. creates_new_application_instance requests isolation where supported.",
        "kill_app" => "Terminate the specified app process. Use only for apps this task owns.",
        "verify_state" => "Poll a specific window for AND-combined predicates. Returns confirmed, failed or unknown; screenshots are evidence for the caller, not interpreted. Element absence cannot yet be proven. timeout_ms:0 samples once.",
        "start_recording" => "Record this session's actions and per-turn evidence in output_dir. Only one recording per session; other sessions are separate. Video is optional (default false); Windows/Linux need ffmpeg. stop_recording flushes the recording.",
        "stop_recording" => "Stop and flush this session's recording; return its output paths and summary.",
        _ => "",
    }
}

fn shorten_schema(schema: &mut Value) {
    match schema {
        Value::Object(object) => {
            if let Some(properties) = object.get_mut("properties").and_then(Value::as_object_mut) {
                properties.remove("session");
                let has_element_target = properties.contains_key("element_token");
                for (name, property) in properties {
                    if name == "target" {
                        if let Some(fields) = property
                            .get_mut("properties")
                            .and_then(Value::as_object_mut)
                        {
                            for field in fields.values_mut() {
                                if let Some(field) = field.as_object_mut() {
                                    field.remove("description");
                                }
                            }
                        }
                    }
                    if name == "scope" {
                        if let Some(property) = property.as_object_mut() {
                            property.remove("description");
                        }
                    }
                    if let Some(text) = property_description(name) {
                        if property.get("description").is_some()
                            && (!matches!(name.as_str(), "pid" | "window_id" | "x" | "y")
                                || has_element_target)
                        {
                            property["description"] = text.into();
                        }
                    }
                }
            }
            if let Some(required) = object.get_mut("required").and_then(Value::as_array_mut) {
                required.retain(|field| field != "session");
            }
            for value in object.values_mut() {
                shorten_schema(value);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(shorten_schema),
        _ => {}
    }
}

fn property_description(name: &str) -> Option<&'static str> {
    Some(match name {
        "element_token" => "Fresh S:N handle from snapshot_id S and row [N]. New reads stale old tokens.",
        "target" => "Exact window: {kind:\"window\",pid,window_id}.",
        "scope" => "Window scope (default).",
        "kind" => "Window target requires pid and window_id.",
        "pid" => "Process id; optional with element_token.",
        "window_id" => "Window id from list_windows; omit with element_token.",
        "x" => "Screenshot-pixel X; pass with y and window_id.",
        "y" => "Screenshot-pixel Y; pass with x and window_id.",
        "since" => "Prior snapshot_id or latest (same window/query/limits). Returns diff with NEW snapshot_id; obey reindexed mappings. Invalid baseline falls back to full read. Null requests full read in run_steps.observe.",
        "tree_format" => "Default markdown; elements for per-row geometry; both includes both representations.",
        "full_output" => "Full metadata and both tree forms; larger walk limits. Incompatible with since.",
        "verbose" => "Include walk diagnostics and background-input metadata. Default false.",
        "capture_mode" => "Deprecated; ignored. Choose element tokens or pixel coordinates at action time.",
        "max_dimension" => "Screenshot long-edge cap in pixels, also bounded by configured ceiling.",
        "max_image_dimension" => "Override screenshot long-edge ceiling in pixels; 0 = native size.",
        "timeout_ms" => return None, // Different semantics and defaults in nested batch checks.
        "cdp_port" => "Explicit CDP port for JavaScript/text actions; otherwise discover from pid.",
        "target_url_contains" => "Require one tab matching this URL substring; window_id alone does not select a CDP tab.",
        "creates_new_application_instance" => "Request a separate app instance/window; single-instance apps may reuse one.",
        "record_video" => "Also record recording.mp4. Default false; Windows/Linux need ffmpeg; macOS needs screen-capture permission.",
        "screenshot_out_file" => "Write the PNG here (~ expanded) instead of embedding it; forces a capture.",
        "css_selector" => "CSS selector for query_dom.",
        "args" => "Tool arguments; role/name/nth/app/window may locate targets instead of ids.",
        "exists" => "Element exists:false is rejected: incomplete walks cannot prove absence. Window predicates support absence.",
        "state_timeout_ms" => "Per-turn accessibility-walk budget in ms; partial walks are marked truncated.",
        "observe" => "One final get_window_state: {} uses last window. Defaults since:latest, include_screenshot:false, max_elements:250. since:null requests full read. Omit to skip.",
        "steps" => "Ordered actions and/or checks; stop at first failure. Each: {tool,args} or {<tool>:args}, optional wait_for/expect/timeout_ms.",
        _ => return None,
    })
}

/// Defaults only on the outward MCP path. Preserve explicit choices, including
/// false. Screenshot-only reads and screenshot_out_file implicitly ask for pixels.
pub fn arguments(name: &str, mut args: Value, brigadier: bool) -> Value {
    if brigadier && name == "get_window_state" {
        let geometry = geometry_for_classification(&args);
        if let Some(object) = args.as_object_mut() {
            let pixels_requested = object.get("include_accessibility_tree")
                == Some(&Value::Bool(false))
                || object
                    .get("screenshot_out_file")
                    .is_some_and(Value::is_string);
            object
                .entry("include_screenshot")
                .or_insert(Value::Bool(pixels_requested));
            if geometry {
                object.insert("tree_format".into(), "both".into());
            }
        }
    } else if brigadier && name == "run_steps" {
        if let Some(observe) = args.get_mut("observe").filter(|value| value.is_object()) {
            if geometry_for_classification(observe) {
                observe["tree_format"] = "both".into();
            }
        }
    }
    args
}

fn geometry_for_classification(args: &Value) -> bool {
    args["include_accessibility_tree"] != false
        && args["full_output"] != true
        && matches!(args["tree_format"].as_str(), None | Some("markdown"))
}

fn restore_read_format(original: &Value, read: &mut Value) {
    if geometry_for_classification(original) {
        if let Some(state) = read
            .get_mut("structuredContent")
            .and_then(Value::as_object_mut)
        {
            state.remove("elements");
            if state.get("tree_format").is_some() {
                state.insert("tree_format".into(), "markdown".into());
            }
        }
    }
}

/// Keep structured trees (both tested clients expose them). Remove only exact
/// duplicate text bodies; headers, token hints, images and metadata survive.
pub fn result(name: &str, mut result: Value, brigadier: bool) -> Value {
    if !brigadier {
        return result;
    }
    if result["isError"] == true {
        // Claude Code errors expose only text. Keep that copy, removing only
        // exact duplicates from structured trees; all error metadata stays.
        let texts: Vec<String> = result["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|block| block["type"] == "text")
            .filter_map(|block| block["text"].as_str().map(str::to_owned))
            .collect();
        if let Some(structured) = result.get_mut("structuredContent") {
            remove_error_tree_duplicates(structured, &texts);
        }
        return result;
    }
    if name == "list_apps" {
        compact_apps(&mut result);
    }
    let mut trees = Vec::new();
    collect_trees(&result["structuredContent"], &mut trees);
    if let Some(content) = result.get_mut("content").and_then(Value::as_array_mut) {
        for block in content.iter_mut() {
            if block["type"] == "text" {
                if let Some(text) = block.get("text").and_then(Value::as_str).map(str::to_owned) {
                    let mut text = text;
                    for tree in &trees {
                        if !tree.is_empty() {
                            text = text.replace(tree, "");
                        }
                    }
                    block["text"] = text.trim().into();
                }
            }
        }
        content.retain(|block| block["type"] != "text" || block["text"] != "");
    }
    result
}

fn collect_trees(value: &Value, trees: &mut Vec<String>) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if matches!(key.as_str(), "tree_markdown" | "tree_diff") {
                    if let Some(text) = value.as_str() {
                        trees.push(text.to_owned());
                    }
                } else if matches!(
                    key.as_str(),
                    "observation" | "state" | "result" | "structuredContent"
                ) {
                    collect_trees(value, trees);
                }
            }
        }
        _ => {}
    }
}

fn remove_error_tree_duplicates(value: &mut Value, texts: &[String]) {
    if let Some(object) = value.as_object_mut() {
        object.retain(|key, value| {
            !matches!(key.as_str(), "tree_markdown" | "tree_diff")
                || !value.as_str().is_some_and(|tree| {
                    !tree.is_empty() && texts.iter().any(|text| text.contains(tree))
                })
        });
        for key in ["observation", "state", "result", "structuredContent"] {
            if let Some(value) = object.get_mut(key) {
                remove_error_tree_duplicates(value, texts);
            }
        }
    }
}

/// An automatic second read captures pixels only, preserving the first read's
/// snapshot and tokens. Explicit false means text only, even for an empty tree.
impl View {
    pub fn fallback_arguments(
        &mut self,
        name: &str,
        original: &Value,
        read: &mut Value,
        brigadier: bool,
    ) -> Option<Value> {
        if brigadier && name == "run_steps" {
            let observation = &read["structuredContent"]["observation"];
            if observation["ok"] != true {
                return None;
            }
            let state = &observation["state"];
            let mut args = original
                .get("observe")
                .filter(|v| v.is_object())
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
            // Use the window actually observed (including failure recovery),
            // rather than guessing from the batch's original last step.
            args["pid"] = state["pid"].clone();
            args["window_id"] = state["window_id"].clone();
            if let Some(session) = original.get("session") {
                args["session"] = session.clone();
            }
            let mut observed =
                serde_json::json!({"structuredContent":state,"content":read["content"]});
            let fallback = self.fallback_arguments("get_window_state", &args, &mut observed, true);
            read["structuredContent"]["observation"]["state"] =
                observed["structuredContent"].take();
            return fallback;
        }
        if !brigadier || name != "get_window_state" || read["isError"] == true {
            return None;
        }
        let window = (original["pid"].as_i64()?, original["window_id"].as_i64()?);
        let state = &read["structuredContent"];
        if state["since_status"] == "no_change" {
            restore_read_format(original, read);
            return None;
        }
        let thin = if state["since_status"] == "diff" {
            let previous = self.windows.get(&window).copied();
            let thin = read_quality::changed_needs_pixels(state, previous);
            let mut quality = previous.unwrap_or_else(|| read_quality::assess(state));
            quality.thin = thin;
            self.windows.insert(window, quality);
            thin
        } else {
            let quality = read_quality::assess(state);
            if quality.unlabelled_controls > 0 {
                read["structuredContent"]["visual_hint"] = format!("{} controls are unlabelled; a screenshot would show them (include_screenshot:true).", quality.unlabelled_controls).into();
            }
            self.windows.insert(window, quality);
            quality.thin
        };
        restore_read_format(original, read);
        if original.get("include_screenshot").is_some()
            || original["include_accessibility_tree"] == false
            || original.get("screenshot_out_file").is_some()
            || read["content"]
                .as_array()
                .is_some_and(|blocks| blocks.iter().any(|b| b["type"] == "image"))
        {
            return None;
        }
        if !thin {
            return None;
        }
        let mut args = original.clone();
        args["include_screenshot"] = true.into();
        args["include_accessibility_tree"] = false.into();
        Some(args)
    }
}

pub fn merge_screenshot(read: &mut Value, image: Value) {
    let has_image = image["content"]
        .as_array()
        .is_some_and(|blocks| blocks.iter().any(|b| b["type"] == "image"));
    let note = if image["isError"] == true || !has_image {
        "Thin accessibility tree; automatic screenshot unavailable. Request include_screenshot:true to retry."
    } else {
        "Thin accessibility tree; screenshot included automatically."
    };
    if let Some(blocks) = image["content"].as_array() {
        if let Some(content) = read["content"].as_array_mut() {
            content.extend(blocks.iter().filter(|b| b["type"] == "image").cloned());
        }
    }
    let is_batch = read
        .pointer("/structuredContent/observation/state")
        .is_some();
    let state = if is_batch {
        &mut read["structuredContent"]["observation"]["state"]
    } else {
        &mut read["structuredContent"]
    };
    if let Some(metadata) = image["structuredContent"].as_object() {
        if let Some(state) = state.as_object_mut() {
            for (key, value) in metadata {
                if key.starts_with("screenshot")
                    || matches!(
                        key.as_str(),
                        "capture_id"
                            | "window_bounds"
                            | "background_input"
                            | "invalidated_snapshot_ids"
                    )
                {
                    state.insert(key.clone(), value.clone());
                }
            }
        }
    }
    if let Some(state) = state.as_object_mut() {
        state.insert("screenshot_note".into(), note.into());
        if image["isError"] == true {
            state.insert("screenshot_failure".into(), image);
        }
    }
}

fn compact_apps(result: &mut Value) {
    let Some(apps) = result
        .pointer("/structuredContent/apps")
        .and_then(Value::as_array)
    else {
        return;
    };
    // Unknown future shapes pass through rather than silently losing entries.
    if apps
        .iter()
        .any(|app| !app.is_object() || !app["name"].is_string())
    {
        return;
    }
    let lines: Vec<Value> = apps
        .iter()
        .map(|app| {
            let name = app["name"].to_string(); // Quotes/escapes keep each record on one line.
            let identity = app.get("bundle_id").filter(|v| v.is_string());
            let identity = identity.map(|id| format!(" {id}")).unwrap_or_default();
            let state = if app["running"] == true {
                format!(
                    " pid={}{}",
                    app["pid"],
                    if app["active"] == true { " active" } else { "" }
                )
            } else {
                " installed".to_owned()
            };
            // Retain launch_path even when bundle_id exists: on Linux the
            // driver ignores bundle_id, and Windows shortcuts carry arguments.
            // Windows, capabilities and future fields are useful evidence too.
            let extra = app
                .as_object()
                .unwrap()
                .iter()
                .filter(|(key, _)| {
                    !matches!(
                        key.as_str(),
                        "name" | "bundle_id" | "running" | "active" | "pid"
                    )
                })
                .map(|(key, value)| format!(" {key}={value}"))
                .collect::<String>();
            Value::String(format!("{name}{identity}{state}{extra}"))
        })
        .collect();
    // Keep text diagnostics, annotations and other content blocks intact.
    result["structuredContent"]["apps"] = lines.into();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fallback_arguments(
        name: &str,
        original: &Value,
        read: &Value,
        brigadier: bool,
    ) -> Option<Value> {
        View::default().fallback_arguments(name, original, &mut read.clone(), brigadier)
    }

    fn read(markdown: &str) -> Value {
        json!({"content":[{"type":"text","text":format!("snapshot_id=s00000001\n{markdown}")}],
            "structuredContent":{"snapshot_id":"s00000001", "tree_markdown":markdown}, "isError":false})
    }

    #[test]
    fn roster_and_nested_session_removal_do_not_change_constraints() {
        let schema = json!({"type":"object","required":["session","pid"],"additionalProperties":false,
            "properties":{"session":{"type":"string"},"pid":{"type":"integer","minimum":1},
            "nested":{"type":"object","required":["session"],"properties":{"session":{"type":"string"}}}}});
        let tools: Vec<Value> = ROSTER
            .iter()
            .chain(["health_report", "start_session"].iter())
            .map(|name| json!({"name":name,"inputSchema":schema,"description":"long description"}))
            .collect();
        let full = json!({"tools":tools});
        assert_eq!(tools_list(full.clone(), false), full);
        let lean = tools_list(full, true);
        assert_eq!(lean["tools"].as_array().unwrap().len(), 20);
        for tool in lean["tools"].as_array().unwrap() {
            assert_eq!(tool["inputSchema"]["required"], json!(["pid"]));
            assert_eq!(tool["inputSchema"]["properties"]["pid"]["minimum"], 1);
            assert_eq!(tool["inputSchema"]["additionalProperties"], false);
            assert!(tool.pointer("/inputSchema/properties/session").is_none());
            assert!(tool
                .pointer("/inputSchema/properties/nested/properties/session")
                .is_none());
        }
    }

    #[test]
    fn defaults_only_when_unset_and_only_for_brigadier_reads() {
        let args = json!({"pid":1,"window_id":2});
        assert_eq!(
            arguments("get_window_state", args.clone(), true)["include_screenshot"],
            false
        );
        assert_eq!(arguments("get_window_state", args.clone(), false), args);
        for name in ["click", "launch_app", "run_steps"] {
            assert_eq!(arguments(name, args.clone(), true), args);
        }
        for explicit in [true, false] {
            let args = json!({"include_screenshot":explicit});
            assert_eq!(
                arguments("get_window_state", args, true)["include_screenshot"],
                explicit
            );
        }
        for args in [
            json!({"include_accessibility_tree":false}),
            json!({"screenshot_out_file":"/tmp/owned.png"}),
        ] {
            assert_eq!(
                arguments("get_window_state", args, true)["include_screenshot"],
                true
            );
        }
    }

    #[test]
    fn tree_once_preserves_header_token_metadata_and_image() {
        let tree = "- [1] AXButton \"Save\"";
        let mut full = read(tree);
        let image = json!({"type":"image","data":"unaltered","mimeType":"image/png"});
        full["content"].as_array_mut().unwrap().push(image.clone());
        full["structuredContent"]["elements"] = json!([{"element_token":"s00000001:1"}]);
        let small = result("get_window_state", full.clone(), true);
        assert_eq!(small["content"][0]["text"], "snapshot_id=s00000001");
        assert_eq!(small["structuredContent"], full["structuredContent"]);
        assert_eq!(small["content"][1], image);
        assert_eq!(
            serde_json::to_string(&small)
                .unwrap()
                .matches("AXButton")
                .count(),
            1
        );
        assert_eq!(result("get_window_state", full.clone(), false), full);
        full["isError"] = true.into();
        let error = result("get_window_state", full.clone(), true);
        assert_eq!(error["content"], full["content"]);
        assert_eq!(error["structuredContent"]["snapshot_id"], "s00000001");
        assert!(error["structuredContent"].get("tree_markdown").is_none());
        assert_eq!(
            serde_json::to_string(&error)
                .unwrap()
                .matches("AXButton")
                .count(),
            1
        );
        let unique_error = json!({"isError":true,"content":[{"type":"text","text":"error text"}],"structuredContent":{"code":"failed","tree_markdown":"unique diagnostic"}});
        assert_eq!(
            result("get_window_state", unique_error.clone(), true),
            unique_error
        );
        let text_error = json!({"isError":true,"content":[{"type":"text","text":"error text"}]});
        assert_eq!(
            result("get_window_state", text_error.clone(), true),
            text_error
        );
    }

    #[test]
    fn batch_observation_diff_is_sent_once() {
        let full = json!({"content":[{"type":"text","text":"step 1 ok"},{"type":"text","text":"~ [2] AXTextField value=new\nreindexed: 1 -> 2"}],
            "structuredContent":{"observation":{"state":{"snapshot_id":"s00000002","tree_diff":"~ [2] AXTextField value=new\nreindexed: 1 -> 2"}}}});
        let small = result("run_steps", full, true);
        assert_eq!(
            small["content"],
            json!([{"type":"text","text":"step 1 ok"}])
        );
        assert_eq!(
            small["structuredContent"]["observation"]["state"]["snapshot_id"],
            "s00000002"
        );
        assert_eq!(
            serde_json::to_string(&small)
                .unwrap()
                .matches("reindexed:")
                .count(),
            1
        );
    }

    #[test]
    fn compact_apps_keeps_installed_apps_and_launch_identity() {
        let full = json!({"content":[{"type":"text","text":"Only Running"}],"structuredContent":{"apps":[
            {"name":"Running","running":true,"active":true,"pid":42,"bundle_id":"org.test.running"},
            {"name":"Installed","running":false,"pid":0,"bundle_id":"org.test.installed"},
            {"name":"Path\nOnly","running":false,"pid":0,"launch_path":"/Apps/Path.app"}]}});
        let small = result("list_apps", full, true);
        let apps = small["structuredContent"]["apps"].as_array().unwrap();
        assert_eq!(apps.len(), 3);
        assert!(apps[0].as_str().unwrap().contains("pid=42 active"));
        assert!(apps[1]
            .as_str()
            .unwrap()
            .contains("org.test.installed\" installed"));
        assert!(apps[2].as_str().unwrap().contains("/Apps/Path.app"));
        assert!(!apps[2].as_str().unwrap().contains('\n'));
    }

    #[test]
    fn thin_real_empty_window_fixture_requests_pixels_without_a_new_tree() {
        let empty: Value =
            serde_json::from_str(include_str!("fixtures/lean-empty-window.json")).unwrap();
        let original = json!({"pid":1,"window_id":2});
        let fallback = fallback_arguments("get_window_state", &original, &empty, true).unwrap();
        assert_eq!(fallback["include_screenshot"], true);
        assert_eq!(fallback["include_accessibility_tree"], false);
        for explicit in [true, false] {
            let args = json!({"pid":1,"window_id":2,"include_screenshot":explicit});
            assert!(fallback_arguments("get_window_state", &args, &empty, true).is_none());
        }
        assert!(fallback_arguments("get_window_state", &original, &empty, false).is_none());
    }

    #[test]
    fn thin_labels_and_unreadable_media_fall_back_but_readable_trees_and_diffs_do_not() {
        // Label-rich 14-row fixture: the audit's smallest normal window was
        // TextEdit (14); Safari/Finder/Settings were 32/97/99 rows respectively.
        let readable = (0..14)
            .map(|i| format!("- [{i}] AXStaticText \"Readable row {i}\"\n"))
            .collect::<String>();
        let original = json!({"pid":1,"window_id":2});
        assert!(
            fallback_arguments("get_window_state", &original, &read(&readable), true).is_none()
        );
        for thin in ["", "- [1] AXButton \"OK\"", "- [1] AXWebArea \"Game\"\n- [2] AXButton \"A\"\n- [3] AXButton \"B\"\n- [4] AXButton \"C\""] {
            assert!(fallback_arguments("get_window_state", &original, &read(thin), true).is_some());
        }
        for status in ["diff", "no_change"] {
            let mut diff = read("");
            diff["structuredContent"]["since_status"] = status.into();
            assert_eq!(
                fallback_arguments("get_window_state", &original, &diff, true).is_some(),
                status == "diff"
            );
        }
    }

    #[test]
    fn automatic_image_preserves_snapshot_and_reports_failed_capture() {
        let mut tree = read("");
        merge_screenshot(
            &mut tree,
            json!({"content":[{"type":"image","data":"pixels"}],
            "structuredContent":{"snapshot_id":"different","capture_id":"new-capture","screenshot":{"width":600}}}),
        );
        assert_eq!(tree["structuredContent"]["snapshot_id"], "s00000001");
        assert_eq!(tree["structuredContent"]["capture_id"], "new-capture");
        assert_eq!(tree["content"][1]["type"], "image");
        assert!(tree["structuredContent"]["screenshot_note"]
            .as_str()
            .unwrap()
            .contains("included automatically"));
        let mut tree = read("");
        merge_screenshot(
            &mut tree,
            json!({"isError":true,"content":[{"type":"text","text":"capture failed"}]}),
        );
        assert!(tree["structuredContent"]["screenshot_note"]
            .as_str()
            .unwrap()
            .contains("unavailable"));
        assert_eq!(tree["structuredContent"]["snapshot_id"], "s00000001");
    }

    #[test]
    fn unchanged_canvas_never_automatically_attaches_pixels() {
        let mut view = View::default();
        let window = json!({"pid":1,"window_id":2});
        assert!(view
            .fallback_arguments(
                "get_window_state",
                &window,
                &mut read("[1] AXWebArea \"Canvas\""),
                true
            )
            .is_some());
        let mut diff =
            json!({"structuredContent":{"since_status":"no_change","tree_diff":"no change"}});
        assert!(view
            .fallback_arguments("get_window_state", &window, &mut diff, true)
            .is_none());
        assert!(view
            .fallback_arguments(
                "get_window_state",
                &json!({"pid":1,"window_id":3}),
                &mut diff,
                true
            )
            .is_none());
    }

    #[test]
    fn visual_content_is_not_explained_by_toolbar_or_caption_text() {
        let original = json!({"pid":1,"window_id":2});
        let mut view = View::default();
        let mut rich = json!({"structuredContent":serde_json::from_str::<Value>(include_str!("fixtures/lean-textedit.json")).unwrap()});
        assert!(view
            .fallback_arguments("get_window_state", &original, &mut rich, true)
            .is_none());
        let mut diff = json!({"structuredContent":{"since_status":"diff", "tree_diff":"+ [5] AXImage \"Photo\"", "elements":[{"element_index":5,"role":"AXImage","frame":{"x":200,"y":160,"w":400,"h":240}}]}});
        assert!(view
            .fallback_arguments("get_window_state", &original, &mut diff, true)
            .is_some());
        let mut unchanged = json!({"structuredContent":{"since_status":"no_change"}});
        assert!(view
            .fallback_arguments("get_window_state", &original, &mut unchanged, true)
            .is_none());
    }

    #[test]
    fn app_compaction_preserves_launch_commands_windows_and_diagnostics() {
        let full = json!({"content":[{"type":"text","text":"partial inventory warning"}, {"type":"image","data":"icon"}],
            "structuredContent":{"apps":[{"name":"Editor","running":true,"pid":42,"bundle_id":"editor.desktop","launch_path":"editor --new-window","windows":[{"window_id":123,"title":"Unsaved"}],"future_field":{"important":true}}]}});
        let small = result("list_apps", full.clone(), true);
        assert_eq!(small["content"], full["content"]);
        let line = small["structuredContent"]["apps"][0].as_str().unwrap();
        for expected in [
            "editor.desktop",
            "editor --new-window",
            "window_id",
            "123",
            "Unsaved",
            "future_field",
            "important",
        ] {
            assert!(line.contains(expected), "{expected}: {line}");
        }
    }

    #[test]
    fn batch_final_observation_gets_pixels_without_repeating_any_steps() {
        let mut batch = json!({"isError":true,"content":[],"structuredContent":{"failed_step":1,"observation":{"ok":true,"state":{"pid":12,"window_id":34,"snapshot_id":"s1","tree_markdown":"[1] AXImage \"Graph\""}}}});
        let original =
            json!({"session":"owned","observe":{},"steps":[{"click":{"pid":99,"window_id":100}}]});
        let fallback = View::default()
            .fallback_arguments("run_steps", &original, &mut batch, true)
            .unwrap();
        assert_eq!(fallback["pid"], 12);
        assert_eq!(fallback["window_id"], 34);
        assert_eq!(fallback["session"], "owned");
        assert_eq!(fallback["include_accessibility_tree"], false);
        assert!(fallback.get("steps").is_none());
        merge_screenshot(
            &mut batch,
            json!({"content":[{"type":"image","data":"pixels"}],"structuredContent":{"capture_id":"frame"}}),
        );
        assert_eq!(batch["content"][0]["type"], "image");
        assert_eq!(
            batch["structuredContent"]["observation"]["state"]["snapshot_id"],
            "s1"
        );
        assert_eq!(
            batch["structuredContent"]["observation"]["state"]["capture_id"],
            "frame"
        );
        assert_eq!(batch["isError"], true);
        let explicit = json!({"observe":{"include_screenshot":false}});
        assert!(View::default()
            .fallback_arguments("run_steps", &explicit, &mut batch, true)
            .is_none());
    }

    #[test]
    fn real_app_fixtures_preserve_visual_only_reads_without_screenshots_on_text_reads() {
        for (name, fixture, expected) in [
            (
                "TextEdit",
                include_str!("fixtures/lean-textedit.json"),
                false,
            ),
            ("Finder", include_str!("fixtures/lean-finder.json"), false),
            ("Safari", include_str!("fixtures/lean-safari.json"), false),
            (
                "Safari canvas",
                include_str!("fixtures/lean-safari-canvas.json"),
                true,
            ),
            ("Preview", include_str!("fixtures/lean-preview.json"), true),
        ] {
            let mut full = json!({"structuredContent":serde_json::from_str::<Value>(fixture).unwrap(),"content":[]});
            let mut view = View::default();
            let args = json!({"pid":1,"window_id":2});
            assert_eq!(
                view.fallback_arguments("get_window_state", &args, &mut full, true)
                    .is_some(),
                expected,
                "{name}"
            );
            assert!(
                full["structuredContent"].get("elements").is_none(),
                "{name}: internal geometry leaked"
            );
            let mut unchanged =
                json!({"structuredContent":{"since_status":"no_change"},"content":[]});
            assert!(
                view.fallback_arguments("get_window_state", &args, &mut unchanged, true)
                    .is_none(),
                "{name}: no-change diff captured pixels"
            );
        }
    }

    #[test]
    fn unlabelled_finder_toolbar_suggests_an_image_without_capturing_one() {
        let mut state: Value =
            serde_json::from_str(include_str!("fixtures/lean-finder.json")).unwrap();
        // Variant of the captured toolbar: remove the four view descriptions,
        // modelling a toolkit that exposes neither labels nor descriptions.
        let mut markdown = state["tree_markdown"].as_str().unwrap().to_owned();
        for description in ["icon view", "list view", "column view", "gallery view"] {
            markdown = markdown.replace(&format!("AXRadioButton ({description})"), "AXRadioButton");
        }
        state["tree_markdown"] = markdown.into();
        for element in state["elements"].as_array_mut().unwrap() {
            if element["role"] == "AXRadioButton" {
                element.as_object_mut().unwrap().remove("label");
            }
        }
        let mut reply = json!({"structuredContent":state,"content":[]});
        assert!(View::default()
            .fallback_arguments(
                "get_window_state",
                &json!({"pid":1,"window_id":2}),
                &mut reply,
                true
            )
            .is_none());
        assert_eq!(
            reply["structuredContent"]["visual_hint"],
            "4 controls are unlabelled; a screenshot would show them (include_screenshot:true)."
        );
        assert!(reply["content"].as_array().unwrap().is_empty());
    }

    #[test]
    fn internal_geometry_preserves_explicit_public_tree_formats() {
        for format in [None, Some("markdown"), Some("elements"), Some("both")] {
            let mut args = json!({"pid":1,"window_id":2});
            if let Some(format) = format {
                args["tree_format"] = format.into();
            }
            let prepared = arguments("get_window_state", args.clone(), true);
            assert_eq!(
                prepared["tree_format"],
                format.filter(|f| *f == "elements").unwrap_or("both")
            );
            let mut reply = json!({"structuredContent":serde_json::from_str::<Value>(include_str!("fixtures/lean-textedit.json")).unwrap()});
            View::default().fallback_arguments("get_window_state", &args, &mut reply, true);
            assert_eq!(
                reply["structuredContent"].get("elements").is_some(),
                matches!(format, Some("elements" | "both"))
            );
        }
        let full = json!({"full_output":true});
        assert!(arguments("get_window_state", full, true)
            .get("tree_format")
            .is_none());
    }

    #[test]
    fn batch_observation_geometry_is_internal_and_no_change_never_captures() {
        let args = json!({"observe":{}});
        assert_eq!(
            arguments("run_steps", args.clone(), true)["observe"]["tree_format"],
            "both"
        );
        assert_eq!(arguments("run_steps", args.clone(), false), args);
        let mut batch = json!({"structuredContent":{"observation":{"ok":true,"state":serde_json::from_str::<Value>(include_str!("fixtures/lean-textedit.json")).unwrap()}},"content":[]});
        batch["structuredContent"]["observation"]["state"]["pid"] = 1.into();
        batch["structuredContent"]["observation"]["state"]["window_id"] = 2.into();
        assert!(View::default()
            .fallback_arguments("run_steps", &args, &mut batch, true)
            .is_none());
        assert!(batch
            .pointer("/structuredContent/observation/state/elements")
            .is_none());
        batch["structuredContent"]["observation"]["state"]["since_status"] = "no_change".into();
        assert!(View::default()
            .fallback_arguments("run_steps", &args, &mut batch, true)
            .is_none());
    }

    #[test]
    fn partial_read_still_gets_pixels_but_explicit_false_and_no_change_win() {
        let mut reply = json!({"structuredContent":serde_json::from_str::<Value>(include_str!("fixtures/lean-textedit.json")).unwrap()});
        reply["structuredContent"]["truncated"] = true.into();
        let args = json!({"pid":1,"window_id":2});
        assert!(View::default()
            .fallback_arguments("get_window_state", &args, &mut reply.clone(), true)
            .is_some());
        assert!(View::default()
            .fallback_arguments(
                "get_window_state",
                &json!({"pid":1,"window_id":2,"include_screenshot":false}),
                &mut reply.clone(),
                true
            )
            .is_none());
        reply["structuredContent"]["since_status"] = "no_change".into();
        assert!(View::default()
            .fallback_arguments("get_window_state", &args, &mut reply, true)
            .is_none());
    }
}
