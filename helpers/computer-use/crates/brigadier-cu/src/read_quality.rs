//! Read quality from accessibility roles, hierarchy and geometry, never label
//! substrings. Shared by AX, UIA and AT-SPI; no extra accessibility walk.

use serde_json::Value;
use std::collections::HashMap;

#[derive(Clone, Copy, Default)]
pub(super) struct Quality {
    pub thin: bool,
    pub unlabelled_controls: usize,
    window: Option<Rect>,
    media: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Role {
    Window,
    Menu,
    Toolbar,
    Container,
    Text,
    Edit,
    Control,
    Media,
    Other,
}

fn role(raw: &str) -> Role {
    let normalized: String = raw
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_lowercase)
        .collect();
    let normalized = normalized.strip_prefix("ax").unwrap_or(&normalized);
    let normalized = normalized.strip_prefix("uia").unwrap_or(normalized);
    let normalized = normalized.strip_prefix("atspirole").unwrap_or(normalized);
    let normalized = normalized.strip_prefix("controltype").unwrap_or(normalized);
    let normalized = normalized
        .strip_suffix("controltypeid")
        .unwrap_or(normalized);
    match normalized {
        "window" | "frame" | "dialog" => Role::Window,
        "menubar" | "menu" | "menuitem" | "menubaritem" => Role::Menu,
        "toolbar" => Role::Toolbar,
        "group" | "pane" | "panel" | "scrollarea" | "scrollpane" | "outline" | "list" | "table"
        | "splitgroup" | "radiogroup" => Role::Container,
        "statictext" | "static" | "text" | "label" | "paragraph" | "row" | "cell" | "tablecell"
        | "dataitem" | "listitem" | "treeitem" | "link" | "hyperlink" | "heading"
        | "headeritem" => Role::Text,
        "textarea" | "textfield" | "edit" | "entry" | "passwordtext" => Role::Edit,
        "button" | "pushbutton" | "menubutton" | "popupbutton" | "splitbutton" | "togglebutton"
        | "checkbox" | "radiobutton" | "combobox" | "slider" | "spinbutton" | "stepper"
        | "tabitem" | "pagetab" | "switch" => Role::Control,
        "image" | "canvas" | "drawingarea" | "video" | "webarea" | "document" | "documentweb"
        | "documentframe" | "documenttext" | "chart" | "graphic" | "embedded"
        | "embeddedobject" | "map" => Role::Media,
        _ => Role::Other,
    }
}

#[derive(Clone, Copy)]
struct Rect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

impl Rect {
    fn from_value(value: &Value) -> Option<Self> {
        let number = |short: &str, long: &str| {
            value
                .get(short)
                .or_else(|| value.get(long))
                .and_then(Value::as_f64)
        };
        let rect = Self {
            x: value["x"].as_f64().unwrap_or(0.0),
            y: value["y"].as_f64().unwrap_or(0.0),
            w: number("w", "width")?,
            h: number("h", "height")?,
        };
        (rect.w.is_finite()
            && rect.h.is_finite()
            && rect.x.is_finite()
            && rect.y.is_finite()
            && rect.w > 0.0
            && rect.h > 0.0)
            .then_some(rect)
    }

    fn substantial_in(self, window: Self) -> bool {
        let w = (self.x + self.w).min(window.x + window.w) - self.x.max(window.x);
        let h = (self.y + self.h).min(window.y + window.h) - self.y.max(window.y);
        // A meaningful visual area, not a title-bar/file/toolbar icon.
        w >= 64.0 && h >= 64.0 && w * h >= window.w * window.h * 0.10
    }
}

struct Node {
    index: Option<u64>,
    parent: Option<usize>,
    role: Role,
    named: bool,
    text: bool,
    caption: bool,
    frame: Option<Rect>,
}

fn nonempty(value: Option<&Value>) -> bool {
    value
        .and_then(Value::as_str)
        .is_some_and(|s| !s.trim().is_empty())
}

fn nodes(state: &Value) -> Vec<Node> {
    let mut nodes: Vec<Node> = Vec::new();
    let mut stack: Vec<(usize, usize)> = Vec::new();
    let is_diff = state["tree_markdown"].is_null();
    if let Some(markdown) = state["tree_markdown"]
        .as_str()
        .or_else(|| state["tree_diff"].as_str())
    {
        for line in markdown.lines() {
            let depth = line.len() - line.trim_start().len();
            let rest = line.trim_start();
            if is_diff && rest.starts_with("- ") {
                continue;
            }
            let rest = rest
                .strip_prefix("- ")
                .or_else(|| rest.strip_prefix("+ "))
                .or_else(|| rest.strip_prefix("~ "))
                .unwrap_or(rest);
            let (index, rest) = if let Some(rest) = rest.strip_prefix('[') {
                let Some((index, rest)) = rest.split_once(']') else {
                    continue;
                };
                (index.parse().ok(), rest.trim_start())
            } else if ["- ", "+ ", "~ "]
                .iter()
                .any(|prefix| line.trim_start().starts_with(prefix))
            {
                (None, rest)
            } else {
                continue;
            };
            let end = rest.find(['"', '(', '[', '=']).unwrap_or(rest.len());
            let raw_role = rest[..end].trim();
            if raw_role.is_empty() {
                continue;
            }
            let text = rest.split('"').nth(1).is_some_and(|s| !s.trim().is_empty());
            let title = text && !rest[..rest.find('"').unwrap_or(rest.len())].contains('=');
            let before_metadata = rest.split(" [").next().unwrap_or(rest);
            let description = before_metadata
                .split_once('(')
                .and_then(|(_, s)| s.split_once(')'))
                .is_some_and(|(s, _)| !s.trim().is_empty());
            let help = rest
                .split_once("help=\"")
                .and_then(|(_, s)| s.split('"').next())
                .is_some_and(|s| !s.trim().is_empty());
            while stack.last().is_some_and(|(d, _)| *d >= depth) {
                stack.pop();
            }
            let parent = stack.last().map(|(_, n)| *n);
            stack.push((depth, nodes.len()));
            nodes.push(Node {
                index,
                parent,
                role: role(raw_role),
                named: title || description || help,
                text: text || description,
                caption: role(raw_role) == Role::Text
                    && state["window_title"]
                        .as_str()
                        .is_some_and(|title| rest.split('"').nth(1) == Some(title)),
                frame: None,
            });
        }
    }
    let mut indices: HashMap<u64, usize> = nodes
        .iter()
        .enumerate()
        .filter_map(|(n, node)| node.index.map(|i| (i, n)))
        .collect();
    if let Some(elements) = state["elements"].as_array() {
        let title = state["window_title"].as_str().or_else(|| {
            elements
                .iter()
                .find(|e| role(e["role"].as_str().unwrap_or_default()) == Role::Window)
                .and_then(|e| e["label"].as_str())
        });
        for element in elements {
            let index = element["element_index"].as_u64();
            let existing = index.and_then(|i| indices.get(&i).copied());
            let n = existing.unwrap_or_else(|| {
                let n = nodes.len();
                let named = ["label", "name", "title", "description", "help", "help_text"]
                    .iter()
                    .any(|key| nonempty(element.get(key)));
                let parent = element["parent_index"]
                    .as_u64()
                    .and_then(|i| indices.get(&i).copied());
                nodes.push(Node {
                    index,
                    parent,
                    role: role(element["role"].as_str().unwrap_or_default()),
                    named,
                    text: named,
                    caption: false,
                    frame: None,
                });
                if let Some(i) = index {
                    indices.insert(i, n);
                }
                n
            });
            nodes[n].frame = Rect::from_value(&element["frame"]);
            nodes[n].text |= nonempty(element.get("value"));
            nodes[n].caption = nodes[n].role == Role::Text
                && title.is_some_and(|title| {
                    element["label"].as_str() == Some(title)
                        || element["value"].as_str() == Some(title)
                });
        }
        // Structured parent links are authoritative when a filtered markdown
        // projection nests a row under the wrong ancestor (audit F10).
        for element in elements {
            if let Some(n) = element["element_index"]
                .as_u64()
                .and_then(|i| indices.get(&i).copied())
            {
                if let Some(parent) = element["parent_index"]
                    .as_u64()
                    .and_then(|i| indices.get(&i).copied())
                    .filter(|&parent| parent < n)
                {
                    nodes[n].parent = Some(parent);
                }
            }
        }
    }
    nodes
}

fn ancestors(nodes: &[Node], n: usize) -> impl Iterator<Item = usize> + '_ {
    std::iter::successors(nodes[n].parent, |&parent| nodes[parent].parent)
}

pub(super) fn assess(state: &Value) -> Quality {
    assess_with_window(state, None)
}

pub(super) fn changed_needs_pixels(state: &Value, previous: Option<Quality>) -> bool {
    let Some(previous) = previous else {
        return true;
    };
    previous.thin || state["truncated"] == true || assess_with_window(state, previous.window).media
}

fn assess_with_window(state: &Value, previous_window: Option<Rect>) -> Quality {
    let nodes = nodes(state);
    let window = Rect::from_value(&state["window_bounds"])
        .or_else(|| {
            nodes
                .iter()
                .find(|n| n.role == Role::Window)
                .and_then(|n| n.frame)
        })
        .or(previous_window);
    let readable = |node: &Node| {
        let title_bar = node.caption
            && node
                .frame
                .zip(window)
                .map(|(frame, window)| frame.y < window.y + 64.0 && frame.h <= 64.0)
                .unwrap_or_else(|| {
                    node.parent
                        .is_some_and(|parent| nodes[parent].role == Role::Window)
                });
        !title_bar
            && (node.role == Role::Edit
                || node.role == Role::Text && node.text
                || node.role == Role::Control && node.named)
    };
    let mut body_text = false;
    let mut body_controls = 0;
    let mut unnamed: HashMap<usize, usize> = HashMap::new();
    let mut large_unreadable_media = false;
    for (n, node) in nodes.iter().enumerate() {
        let chrome =
            ancestors(&nodes, n).any(|a| matches!(nodes[a].role, Role::Menu | Role::Toolbar));
        if !chrome {
            body_text |= readable(node) && matches!(node.role, Role::Text | Role::Edit);
            body_controls += usize::from(node.named && node.role == Role::Control);
        }
        if (node.role == Role::Control || node.role == Role::Edit && !node.text)
            && !node.named
            && node
                .frame
                .is_some_and(|frame| frame.w >= 12.0 && frame.h >= 12.0)
        {
            // Window close/minimize/zoom buttons are not a toolbar cluster.
            if let Some(group) = ancestors(&nodes, n)
                .find(|&a| matches!(nodes[a].role, Role::Toolbar | Role::Container))
            {
                *unnamed.entry(group).or_default() += 1;
            }
        }
        if node.role == Role::Media
            && node
                .frame
                .zip(window)
                .is_some_and(|(frame, window)| frame.substantial_in(window))
        {
            let children_readable = nodes.iter().enumerate().any(|(child, item)| {
                readable(item)
                    && ancestors(&nodes, child).any(|a| a == n)
                    && !ancestors(&nodes, child)
                        .take_while(|&a| a != n)
                        .any(|a| matches!(nodes[a].role, Role::Menu | Role::Toolbar))
            });
            large_unreadable_media |= !children_readable;
        }
    }
    let unlabelled_controls = unnamed.values().filter(|&&n| n >= 3).sum();
    // Toolbar-only image documents and inaccessible/empty windows still need
    // pixels even if the backend gives no image node/frame. An unlabelled
    // control cluster alone asks for a screenshot instead of forcing one.
    Quality {
        thin: state["truncated"] == true
            || large_unreadable_media
            || (!body_text && body_controls < 4 && unlabelled_controls == 0),
        unlabelled_controls,
        window,
        media: large_unreadable_media,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn accessibility_role_names_are_normalized_across_platforms() {
        for name in [
            "AXStaticText",
            "AXRow",
            "AXCell",
            "ControlType.Text",
            "UIA_TextControlTypeId",
            "DataItem",
            "table cell",
            "ATSPI_ROLE_TEXT",
            "list item",
            "paragraph",
        ] {
            assert_eq!(role(name), Role::Text, "{name}");
        }
        for name in ["AXTextArea", "AXTextField", "ControlType.Edit", "entry"] {
            assert_eq!(role(name), Role::Edit, "{name}");
        }
        for name in [
            "AXWebArea",
            "ControlType.Document",
            "document web",
            "drawing area",
            "AXImage",
            "video",
        ] {
            assert_eq!(role(name), Role::Media, "{name}");
        }
    }

    #[test]
    fn only_large_media_without_readable_descendants_needs_pixels() {
        let mut state = json!({"window_bounds":{"x":0,"y":0,"width":1000,"height":800},"elements":[
            {"element_index":0,"role":"Window"},
            {"element_index":1,"parent_index":0,"role":"Text","label":"A document about canvas and video"},
            {"element_index":2,"parent_index":0,"role":"Image","frame":{"x":0,"y":0,"w":32,"h":32}}]});
        assert!(!assess(&state).thin);
        state["elements"][2]["frame"] = json!({"x":0,"y":100,"w":400,"h":300});
        assert!(assess(&state).thin);
        state["elements"].as_array_mut().unwrap().push(
            json!({"element_index":3,"parent_index":2,"role":"Text","label":"Readable content"}),
        );
        assert!(!assess(&state).thin);
        state["elements"][2]["frame"] = json!({"x":2000,"y":100,"w":400,"h":300});
        state["elements"].as_array_mut().unwrap().pop();
        assert!(!assess(&state).thin, "off-window image");
    }
}
