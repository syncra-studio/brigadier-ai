//! The structure part of an observation (§4.3): the pruned tree, refs that stay stable while the
//! element is the same, generations that catch recycled and replaced elements, and the compact
//! text a model reads, full or as a diff against what this worker saw last.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::hash::Hash;

use serde::{Deserialize, Serialize};

use crate::geom::Rect;

/// A checkbox-like state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Check {
    On,
    Off,
    Mixed,
}

/// One element as a backend reads it, in pre-order with its depth.
#[derive(Debug, Clone, PartialEq)]
pub struct RawNode<E> {
    pub element: E,
    pub depth: u16,
    /// A short, platform-neutral role: `button`, `checkbox`, `textfield`, `text`, `group`…
    pub role: String,
    pub label: Option<String>,
    /// Never set for a secure field: backends drop it, and the renderer would too.
    pub value: Option<String>,
    /// In window points.
    pub frame: Option<Rect>,
    pub enabled: bool,
    pub focused: bool,
    pub selected: bool,
    pub checked: Option<Check>,
    pub expanded: Option<bool>,
    pub secure: bool,
    /// Platform-neutral action names beyond the role's default (`show-menu`, `increment`…).
    pub actions: Vec<String>,
    /// Out of view and not read: only its role is known.
    pub unread: bool,
}

impl<E> RawNode<E> {
    pub fn new(element: E, depth: u16, role: &str) -> Self {
        Self {
            element,
            depth,
            role: role.to_owned(),
            label: None,
            value: None,
            frame: None,
            enabled: true,
            focused: false,
            selected: false,
            checked: None,
            expanded: None,
            secure: false,
            actions: Vec::new(),
            unread: false,
        }
    }
}

/// Roles a model acts on even when they carry no label.
const INTERACTIVE: &[&str] = &[
    "button",
    "checkbox",
    "radio",
    "textfield",
    "secure-field",
    "text-area",
    "slider",
    "stepper",
    "popup",
    "combo",
    "menu-button",
    "link",
    "tab",
    "row",
    "cell",
    "menu-item",
    "disclosure",
    "scroll",
    "table",
    "outline",
    "list",
    "web",
    "canvas",
    "sheet",
    "window",
    "toolbar",
    "tabs",
    "menu-bar-item",
    "search-field",
    "color-well",
    "date-field",
];

/// Whether an element earns a line of its own.
fn keep<E>(n: &RawNode<E>) -> bool {
    if n.unread {
        return true;
    }
    let named = n.label.as_deref().is_some_and(|l| !l.trim().is_empty());
    let valued = n.value.as_deref().is_some_and(|v| !v.trim().is_empty());
    let stateful =
        n.focused || n.selected || n.checked.is_some() || n.expanded.is_some() || n.secure;
    if n.role == "group" || n.role == "unknown" || n.role == "split-group" || n.role == "layout" {
        return named;
    }
    named || valued || stateful || !n.actions.is_empty() || INTERACTIVE.contains(&n.role.as_str())
}

/// An unlabelled row, cell or group whose only kept content is one text leaf shows that text
/// on its own line, and the leaf gets no line of its own: `row "Row 3"` instead of two lines.
fn merge_child<E>(nodes: &[RawNode<E>], kept: &[bool], i: usize, end: usize) -> Option<usize> {
    let n = &nodes[i];
    if !kept[i]
        || n.label.is_some()
        || n.value.is_some()
        || !matches!(n.role.as_str(), "row" | "cell" | "group" | "list-item")
    {
        return None;
    }
    let mut inner = (i + 1..end).filter(|&j| kept[j]);
    let only = inner.next()?;
    if inner.next().is_some() {
        return None;
    }
    let c = &nodes[only];
    let leaf = matches!(c.role.as_str(), "text" | "textfield" | "cell" | "image");
    (leaf && !c.secure && (c.label.is_some() || c.value.is_some())).then_some(only)
}

/// How long a value is shown before it is clipped.
const VALUE_CLIP: usize = 80;

fn quote(s: &str, max: usize) -> String {
    let flat: String = s
        .chars()
        .map(|c| {
            if c == '\n' || c == '\r' || c == '\t' {
                ' '
            } else {
                c
            }
        })
        .collect();
    let n = flat.chars().count();
    if n <= max {
        format!("{flat:?}")
    } else {
        let cut: String = flat.chars().take(max).collect();
        format!("{:?}… ({n} chars)", cut)
    }
}

/// The line a kept element renders to, without its ref and indentation.
pub fn render_line<E>(n: &RawNode<E>) -> String {
    let mut s = n.role.clone();
    if let Some(l) = n.label.as_deref().filter(|l| !l.trim().is_empty()) {
        let _ = write!(s, " {}", quote(l, VALUE_CLIP));
    }
    if n.secure {
        s.push_str(" value=<hidden>");
    } else if let Some(v) = n.value.as_deref().filter(|v| !v.is_empty())
        && n.label.as_deref() != Some(v)
    {
        let _ = write!(s, " value={}", quote(v, VALUE_CLIP));
    }
    match n.checked {
        Some(Check::On) => s.push_str(" checked"),
        Some(Check::Mixed) => s.push_str(" mixed"),
        Some(Check::Off) => s.push_str(" unchecked"),
        None => {}
    }
    if n.expanded == Some(true) {
        s.push_str(" expanded");
    }
    if n.focused {
        s.push_str(" focused");
    }
    if n.selected {
        s.push_str(" selected");
    }
    if !n.enabled {
        s.push_str(" disabled");
    }
    if !n.actions.is_empty() {
        let _ = write!(s, " actions={}", n.actions.join(","));
    }
    if let Some(f) = n.frame {
        let _ = write!(
            s,
            " @{},{} {}x{}",
            f.x.round() as i64,
            f.y.round() as i64,
            f.w.round() as i64,
            f.h.round() as i64
        );
    }
    s
}

/// What a ref remembers about its element, re-checked before every action (§4.3).
#[derive(Debug, Clone)]
pub struct RefRecord<E> {
    pub element: E,
    pub role: String,
    pub label: Option<String>,
    pub enabled: bool,
    pub frame: Option<Rect>,
    pub secure: bool,
    /// The visible area around it when it was seen: the window narrowed by its scroll views.
    pub clip: Option<Rect>,
    /// The window's content generation when the ref was last seen.
    pub generation: u64,
}

/// One rendered element of an observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub r: u32,
    pub depth: u16,
    pub text: String,
    /// The nearest kept ancestor, for paging and `find`.
    pub parent: Option<u32>,
    /// Scrolled out of view: left out unless asked for.
    pub hidden: bool,
}

/// The refs of one window.
#[derive(Debug)]
pub struct WindowRefs<E> {
    refs: HashMap<E, u32>,
    records: HashMap<u32, RefRecord<E>>,
    next: u32,
    /// Bumped when the window navigates: refs from before are stale.
    pub generation: u64,
}

impl<E> Default for WindowRefs<E> {
    fn default() -> Self {
        Self {
            refs: HashMap::new(),
            records: HashMap::new(),
            next: 1,
            generation: 0,
        }
    }
}

impl<E: Clone + Eq + Hash> WindowRefs<E> {
    /// Gives every kept node a ref (keeping the ref of an element seen before) and returns the
    /// rendered lines. Elements gone from the tree lose their refs.
    pub fn assign(&mut self, nodes: &[RawNode<E>]) -> Vec<Line> {
        let kept: Vec<bool> = nodes.iter().map(keep).collect();
        // Where each node's subtree ends.
        let mut end = vec![nodes.len(); nodes.len()];
        let mut open: Vec<usize> = Vec::new();
        for (i, n) in nodes.iter().enumerate() {
            while open.last().is_some_and(|&j| nodes[j].depth >= n.depth) {
                if let Some(j) = open.pop() {
                    end[j] = i;
                }
            }
            open.push(i);
        }
        let mut merged_into: HashMap<usize, usize> = HashMap::new();
        for (i, &stop) in end.iter().enumerate() {
            if let Some(child) = merge_child(nodes, &kept, i, stop) {
                merged_into.insert(child, i);
            }
        }
        let mut lines = Vec::new();
        // The kept ancestors: (raw depth, ref, kept depth).
        let mut stack: Vec<(u16, u32, u16)> = Vec::new();
        // The visible area: the window, narrowed by every scroll view on the way down.
        let mut clips: Vec<(u16, Rect)> = Vec::new();
        let mut seen: HashMap<E, u32> = HashMap::with_capacity(nodes.len());
        for (i, n) in nodes.iter().enumerate() {
            while stack.last().is_some_and(|&(d, _, _)| d >= n.depth) {
                stack.pop();
            }
            while clips.last().is_some_and(|&(d, _)| d >= n.depth) {
                clips.pop();
            }
            let clip = clips.last().map(|&(_, c)| c);
            let hidden = n.unread
                || match (clip, n.frame) {
                    (Some(c), Some(f)) => f.intersect(&c).is_empty(),
                    _ => false,
                };
            if (n.role == "scroll" || n.role == "window")
                && let Some(f) = n.frame
            {
                clips.push((n.depth, clip.map_or(f, |c| c.intersect(&f))));
            }
            if !kept[i] || merged_into.contains_key(&i) {
                continue;
            }
            let r = match self.refs.get(&n.element) {
                Some(&r) => r,
                None => {
                    let r = self.next;
                    self.next += 1;
                    r
                }
            };
            seen.insert(n.element.clone(), r);
            // An unread element keeps what an earlier full read learned about it.
            if n.unread
                && let Some(rec) = self.records.get_mut(&r)
            {
                rec.generation = self.generation;
                lines.push(Line {
                    r,
                    depth: stack.last().map_or(0, |&(_, _, k)| k + 1),
                    text: render_line(n),
                    parent: stack.last().map(|&(_, r, _)| r),
                    hidden,
                });
                continue;
            }
            self.records.insert(
                r,
                RefRecord {
                    element: n.element.clone(),
                    role: n.role.clone(),
                    label: n.label.clone(),
                    enabled: n.enabled,
                    frame: n.frame,
                    secure: n.secure,
                    clip,
                    generation: self.generation,
                },
            );
            let parent = stack.last().map(|&(_, r, _)| r);
            let depth = stack.last().map_or(0, |&(_, _, k)| k + 1);
            let text = match merged_into.iter().find(|&(_, &into)| into == i) {
                Some((&child, _)) => {
                    let c = &nodes[child];
                    let mut shown = n.clone();
                    shown.label = c.label.clone().or_else(|| c.value.clone());
                    render_line(&shown)
                }
                None => render_line(n),
            };
            lines.push(Line {
                r,
                depth,
                text,
                parent,
                hidden,
            });
            stack.push((n.depth, r, depth));
        }
        let live: std::collections::HashSet<u32> = seen.values().copied().collect();
        self.records.retain(|r, _| live.contains(r));
        self.refs = seen;
        lines
    }

    pub fn get(&self, r: u32) -> Option<&RefRecord<E>> {
        self.records.get(&r)
    }

    /// The frames of the password fields seen last, to paint over in images.
    pub fn secure_frames(&self) -> Vec<Rect> {
        self.records
            .values()
            .filter(|r| r.secure)
            .filter_map(|r| r.frame)
            .collect()
    }

    /// The window navigated: every ref handed out so far is stale.
    pub fn navigate(&mut self) {
        self.generation += 1;
    }
}

/// Parses `e12` (or `12`) into a ref number.
pub fn parse_ref(s: &str) -> Option<u32> {
    s.strip_prefix('e').unwrap_or(s).parse().ok()
}

/// What `render` should include.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    /// Only this element and what is under it.
    pub element: Option<u32>,
    /// Only lines containing this text (case-insensitive), plus their ancestors.
    pub find: Option<String>,
}

fn select(lines: &[Line], f: &Filter) -> Vec<bool> {
    let mut on = vec![f.element.is_none() && f.find.is_none(); lines.len()];
    let index: HashMap<u32, usize> = lines.iter().enumerate().map(|(i, l)| (l.r, i)).collect();
    if let Some(root) = f.element {
        let mut inside: Vec<u32> = Vec::new();
        for (i, l) in lines.iter().enumerate() {
            let under = l.r == root || l.parent.is_some_and(|p| inside.contains(&p));
            if under {
                inside.push(l.r);
                on[i] = true;
            }
        }
    }
    if let Some(needle) = f.find.as_deref().map(str::to_lowercase) {
        let scope = on.clone();
        let scoped = f.element.is_some();
        on = vec![false; lines.len()];
        for (i, l) in lines.iter().enumerate() {
            if (!scoped || scope[i]) && l.text.to_lowercase().contains(&needle) {
                on[i] = true;
                let mut p = l.parent;
                while let Some(r) = p {
                    let Some(&j) = index.get(&r) else { break };
                    on[j] = true;
                    p = lines[j].parent;
                }
            }
        }
    }
    on
}

/// Renders lines in full, within `budget` characters. What doesn't fit is named by where it
/// is, so the model can ask for that part (§4.3: paging, not truncation).
pub fn render_full(lines: &[Line], filter: &Filter, budget: usize) -> String {
    let on = select(lines, filter);
    let base_depth = lines
        .iter()
        .zip(&on)
        .filter(|(_, o)| **o)
        .map(|(l, _)| l.depth)
        .min()
        .unwrap_or(0);
    let mut out = String::new();
    let mut left_out: BTreeMap<Option<u32>, usize> = BTreeMap::new();
    let mut out_of_view: BTreeMap<Option<u32>, usize> = BTreeMap::new();
    let filtered = filter.element.is_some() || filter.find.is_some();
    for (l, _) in lines.iter().zip(&on).filter(|(_, o)| **o) {
        if l.hidden && !filtered {
            *out_of_view.entry(l.parent).or_default() += 1;
            continue;
        }
        let line = format!(
            "{}e{} {}\n",
            "  ".repeat(usize::from(l.depth - base_depth)),
            l.r,
            l.text
        );
        if out.len() + line.len() > budget || !left_out.is_empty() {
            *left_out.entry(l.parent).or_default() += 1;
            continue;
        }
        out.push_str(&line);
    }
    for (parent, n) in out_of_view {
        if let Some(p) = parent {
            let _ = writeln!(out, "… {n} out of view under e{p}: observe element e{p}");
        }
    }
    for (parent, n) in left_out {
        match parent {
            Some(p) => {
                let _ = writeln!(out, "… {n} more under e{p}: observe element e{p}");
            }
            None => {
                let _ = writeln!(out, "… {n} more at the top level: observe with find");
            }
        }
    }
    out
}

/// Renders what changed since `base` (ref → line text): `~` changed, `+` added, `-` ranges of
/// removed refs. One line when nothing changed.
pub fn render_diff(lines: &[Line], base: &HashMap<u32, String>, budget: usize) -> String {
    let mut out = String::new();
    let mut omitted = 0usize;
    let mut push = |out: &mut String, s: String| {
        if out.len() + s.len() > budget {
            omitted += 1;
        } else {
            out.push_str(&s);
        }
    };
    for l in lines {
        match base.get(&l.r) {
            Some(old) if *old == l.text => {}
            Some(_) => push(&mut out, format!("~ e{} {}\n", l.r, l.text)),
            None => push(&mut out, format!("+ e{} {}\n", l.r, l.text)),
        }
    }
    let now: std::collections::HashSet<u32> = lines.iter().map(|l| l.r).collect();
    let mut gone: Vec<u32> = base.keys().copied().filter(|r| !now.contains(r)).collect();
    gone.sort_unstable();
    for (a, b) in ranges(&gone) {
        let s = if a == b {
            format!("- e{a}\n")
        } else {
            format!("- e{a}–e{b}\n")
        };
        push(&mut out, s);
    }
    if omitted > 0 {
        let _ = writeln!(out, "… {omitted} more changes: observe full");
    }
    if out.is_empty() {
        out.push_str("no change\n");
    }
    out
}

fn ranges(sorted: &[u32]) -> Vec<(u32, u32)> {
    let mut out: Vec<(u32, u32)> = Vec::new();
    for &r in sorted {
        match out.last_mut() {
            Some((_, b)) if *b + 1 == r => *b = r,
            _ => out.push((r, r)),
        }
    }
    out
}

/// A rough token count for text: 4 characters a token.
pub fn text_tokens(s: &str) -> usize {
    s.len().div_ceil(4)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: u32, depth: u16, role: &str, label: Option<&str>) -> RawNode<u32> {
        let mut n = RawNode::new(id, depth, role);
        n.label = label.map(str::to_owned);
        n.frame = Some(Rect::new(f64::from(id), 0.0, 10.0, 10.0));
        n
    }

    fn sample() -> Vec<RawNode<u32>> {
        vec![
            node(1, 0, "window", Some("Doc")),
            node(2, 1, "group", None),
            node(3, 2, "group", None),
            node(4, 3, "button", Some("OK")),
            node(5, 3, "text", None),
            node(6, 1, "checkbox", Some("Bold")),
        ]
    }

    #[test]
    fn unnamed_containers_collapse_and_kept_elements_rise() {
        let mut refs = WindowRefs::default();
        let lines = refs.assign(&sample());
        let text = render_full(&lines, &Filter::default(), 10_000);
        assert_eq!(
            text,
            "e1 window \"Doc\" @1,0 10x10\n  e2 button \"OK\" @4,0 10x10\n  e3 checkbox \"Bold\" @6,0 10x10\n"
        );
    }

    #[test]
    fn refs_survive_a_reobserve_and_new_elements_get_new_ones() {
        let mut refs = WindowRefs::default();
        refs.assign(&sample());
        let mut next = sample();
        next.insert(4, node(9, 3, "button", Some("Cancel")));
        let lines = refs.assign(&next);
        let ok = lines.iter().find(|l| l.text.contains("OK")).unwrap();
        let cancel = lines.iter().find(|l| l.text.contains("Cancel")).unwrap();
        assert_eq!(ok.r, 2);
        assert_eq!(cancel.r, 4);
    }

    #[test]
    fn a_ref_remembers_role_label_and_generation() {
        let mut refs = WindowRefs::default();
        refs.assign(&sample());
        let rec = refs.get(2).unwrap();
        assert_eq!(
            (rec.role.as_str(), rec.label.as_deref(), rec.generation),
            ("button", Some("OK"), 0)
        );
        refs.navigate();
        assert_eq!(
            refs.get(2).unwrap().generation,
            0,
            "the old ref keeps its old generation"
        );
        assert_eq!(refs.generation, 1);
    }

    #[test]
    fn gone_elements_lose_their_refs() {
        let mut refs = WindowRefs::default();
        refs.assign(&sample());
        let mut next = sample();
        next.retain(|n| n.element != 4);
        refs.assign(&next);
        assert!(refs.get(2).is_none());
    }

    #[test]
    fn diffs_show_changes_additions_and_removed_ranges() {
        let mut refs = WindowRefs::default();
        let first = refs.assign(&sample());
        let base: HashMap<u32, String> = first.iter().map(|l| (l.r, l.text.clone())).collect();
        assert_eq!(render_diff(&first, &base, 10_000), "no change\n");
        let mut next = sample();
        next[5].checked = Some(Check::On);
        next.retain(|n| n.element != 4);
        next.push(node(7, 1, "button", Some("Apply")));
        let lines = refs.assign(&next);
        let d = render_diff(&lines, &base, 10_000);
        assert_eq!(
            d,
            "~ e3 checkbox \"Bold\" checked @6,0 10x10\n+ e4 button \"Apply\" @7,0 10x10\n- e2\n"
        );
    }

    #[test]
    fn over_budget_output_says_where_the_rest_is() {
        let mut nodes = vec![node(1, 0, "table", Some("Rows"))];
        for i in 0..200 {
            nodes.push(node(100 + i, 1, "row", Some(&format!("Row {i}"))));
        }
        let mut refs = WindowRefs::default();
        let lines = refs.assign(&nodes);
        let text = render_full(&lines, &Filter::default(), 600);
        assert!(text.len() < 700);
        assert!(
            text.ends_with("more under e1: observe element e1\n"),
            "{text}"
        );
        let sub = render_full(
            &lines,
            &Filter {
                element: Some(1),
                find: Some("Row 173".into()),
            },
            600,
        );
        assert!(sub.contains("Row 173") && sub.contains("e1 table"), "{sub}");
    }

    #[test]
    fn secure_values_never_render_and_long_values_say_their_length() {
        let mut n = node(1, 0, "secure-field", Some("Password"));
        n.secure = true;
        n.value = Some("hunter2".into());
        assert!(!render_line(&n).contains("hunter2"));
        let mut t = node(2, 0, "text-area", None);
        t.value = Some("x".repeat(500));
        assert!(render_line(&t).contains("(500 chars)"));
    }
}
