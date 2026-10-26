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

/// Small diffs do not tell us whether a canvas changed visually. Remember the
/// last full tree for each window so thin surfaces keep getting fresh pixels.
#[derive(Default)]
pub struct View {
    thin_windows: HashMap<(i64, i64), bool>,
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
        if let Some(object) = args.as_object_mut() {
            let pixels_requested = object.get("include_accessibility_tree")
                == Some(&Value::Bool(false))
                || object
                    .get("screenshot_out_file")
                    .is_some_and(Value::is_string);
            object
                .entry("include_screenshot")
                .or_insert(Value::Bool(pixels_requested));
        }
    }
    args
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
        read: &Value,
        brigadier: bool,
    ) -> Option<Value> {
        if !brigadier || name != "get_window_state" || read["isError"] == true {
            return None;
        }
        let window = (original["pid"].as_i64()?, original["window_id"].as_i64()?);
        let state = &read["structuredContent"];
        let is_diff = matches!(state["since_status"].as_str(), Some("diff" | "no_change"));
        let thin = if is_diff {
            // A diff cannot prove the rest of the window is text-complete.
            // Navigation can introduce a canvas after a previously rich read.
            let thin = self.thin_windows.get(&window).copied().unwrap_or(true)
                || state["tree_diff"].as_str().is_some_and(has_visual_surface);
            self.thin_windows.insert(window, thin);
            thin
        } else {
            let thin = thin_state(state);
            self.thin_windows.insert(window, thin);
            thin
        };
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

fn thin_state(state: &Value) -> bool {
    if let Some(markdown) = state["tree_markdown"].as_str() {
        thin_tree(markdown)
    } else if let Some(elements) = state["elements"].as_array() {
        let markdown = elements
            .iter()
            .enumerate()
            .map(|(index, element)| {
                format!(
                    "[{index}] {} {:?} value={:?}\n",
                    element["role"].as_str().unwrap_or_default(),
                    element
                        .get("label")
                        .or_else(|| element.get("title"))
                        .or_else(|| element.get("name"))
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                    element
                        .get("value")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                )
            })
            .collect::<String>();
        thin_tree(&markdown)
    } else {
        // Missing/unknown tree shapes cannot establish readable content.
        true
    }
}

// Audit §2: the smallest useful normal read is TextEdit (14 rows), then
// Safari (32), Finder (97), Settings (99). Use a conservative lower threshold
// of fewer than four labelled rows, not total nodes: container-only trees
// can have many rows. Media containers need actual readable content as well.
fn thin_tree(markdown: &str) -> bool {
    if has_visual_surface(markdown) {
        return true;
    }
    let mut labelled = 0;
    let mut media = false;
    let mut readable = false;
    for line in markdown.lines() {
        let Some((_, rest)) = line.split_once(']') else {
            continue;
        };
        let lower = rest.to_lowercase();
        let label = rest.split('"').nth(1).unwrap_or_default().trim();
        let has_text = !label.is_empty()
            || rest
                .split_once("value=")
                .is_some_and(|(_, v)| !v.trim_matches([' ', '"']).is_empty());
        labelled += usize::from(has_text);
        media |= [
            "axwebarea",
            "webarea",
            "web area",
            "document",
            "canvas",
            "drawing area",
            "drawingarea",
            "aximage",
            " image",
        ]
        .iter()
        .any(|role| lower.contains(role));
        readable |= has_text
            && [
                "statictext",
                "textarea",
                "textfield",
                "axlink",
                "hyperlink",
                "link",
                "entry",
                "label",
                "paragraph",
                " text",
                " edit",
            ]
            .iter()
            .any(|role| lower.trim_start().starts_with(role.trim_start()));
    }
    labelled < 4 || (media && !readable)
}

/// Readable browser chrome or captions do not describe the pixels of a
/// drawing, photo, video, chart or embedded surface. Look only at the role,
/// so ordinary prose containing "image" does not trigger a capture.
fn has_visual_surface(markdown: &str) -> bool {
    markdown.lines().any(|line| {
        let Some((_, rest)) = line.split_once(']') else {
            return false;
        };
        let role = rest
            .split('"')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        [
            "canvas",
            "drawingarea",
            "drawing area",
            "image",
            "video",
            "chart",
            "graphic",
            "embedded",
            "map",
        ]
        .iter()
        .any(|visual| role.trim_start_matches("ax").starts_with(visual))
    })
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
    if let Some(metadata) = image["structuredContent"].as_object() {
        if let Some(state) = read["structuredContent"].as_object_mut() {
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
    if let Some(state) = read["structuredContent"].as_object_mut() {
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
        View::default().fallback_arguments(name, original, read, brigadier)
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
            assert_eq!(arguments("get_window_state", args.clone(), true), args);
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
            assert!(fallback_arguments("get_window_state", &original, &diff, true).is_some());
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
    fn unchanged_canvas_still_gets_pixels_without_affecting_other_windows() {
        let mut view = View::default();
        let window = json!({"pid":1,"window_id":2});
        assert!(view
            .fallback_arguments(
                "get_window_state",
                &window,
                &read("[1] AXWebArea \"Canvas\""),
                true
            )
            .is_some());
        let diff =
            json!({"structuredContent":{"since_status":"no_change","tree_diff":"no change"}});
        assert!(view
            .fallback_arguments("get_window_state", &window, &diff, true)
            .is_some());
        assert!(view
            .fallback_arguments(
                "get_window_state",
                &json!({"pid":1,"window_id":3}),
                &diff,
                true
            )
            .is_some());
    }

    #[test]
    fn visual_content_is_not_explained_by_toolbar_or_caption_text() {
        let chrome = "[1] AXTextField \"Address\"\n[2] AXStaticText \"Ready\"\n[3] AXButton \"Save\"\n[4] AXButton \"Open\"\n";
        for role in [
            "AXImage",
            "AXCanvas",
            "Image",
            "drawing area",
            "video",
            "chart",
        ] {
            assert!(
                thin_tree(&format!("{chrome}[5] {role} \"content\"")),
                "{role}"
            );
        }
        assert!(!thin_tree(&format!(
            "{chrome}[5] AXStaticText \"image processing\""
        )));
        let original = json!({"pid":1,"window_id":2});
        let mut view = View::default();
        assert!(view
            .fallback_arguments("get_window_state", &original, &read(chrome), true)
            .is_none());
        let diff = json!({"structuredContent":{"since_status":"diff", "tree_diff":"+ [5] AXImage \"Photo\""}});
        assert!(view
            .fallback_arguments("get_window_state", &original, &diff, true)
            .is_some());
        let unchanged = json!({"structuredContent":{"since_status":"no_change"}});
        assert!(view
            .fallback_arguments("get_window_state", &original, &unchanged, true)
            .is_some());
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
    fn native_windows_and_linux_roles_are_classified_without_ax_prefixes() {
        let chrome = "[1] Button \"Back\"\n[2] Button \"Forward\"\n[3] Button \"Reload\"\n[4] Button \"Menu\"\n";
        assert!(thin_tree(&format!("{chrome}[5] Document \"Canvas\"")));
        assert!(thin_tree(&format!("{chrome}[5] document web \"Game\"")));
        for role in ["Text", "Edit", "entry", "link", "paragraph"] {
            assert!(
                !thin_tree(&format!(
                    "{chrome}[5] document web \"Page\"\n[6] {role} \"Readable content\""
                )),
                "{role}"
            );
        }
    }
}
