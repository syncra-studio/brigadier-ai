//! Input that reaches the page itself: mouse, wheel, keys and text, sent to the tab's session.
//! The browser routes them as it routes a person's, frames included, and marks them trusted;
//! the operating system never sees them, so the user's focus and cursor stay where they are.

use std::time::Duration;

use serde_json::{Value, json};

use super::conn::Conn;
use super::keys::{PageKey, mac_command, modifier_bits, page_key};
use super::page::{Page, WebEl, call_on, content_quad};
use crate::cancel::CancelToken;
use crate::desktop::{Button, Chord, Mods};
use crate::error::{CuError, CuResult, ErrorCode, err};
use crate::geom::Rect;

fn button_name(b: Button) -> &'static str {
    match b {
        Button::Left => "left",
        Button::Right => "right",
        Button::Middle => "middle",
    }
}

/// A click (or a double or triple click) at a point of the main viewport, CSS pixels.
pub fn click(
    conn: &mut Conn,
    page: &Page,
    (x, y): (f64, f64),
    button: Button,
    count: u8,
    mods: Mods,
) -> CuResult<()> {
    let s = Some(page.session.as_str());
    let m = modifier_bits(mods);
    let b = button_name(button);
    conn.call(
        s,
        "Input.dispatchMouseEvent",
        json!({"type": "mouseMoved", "x": x, "y": y, "modifiers": m}),
    )?;
    for n in 1..=count.max(1) {
        for kind in ["mousePressed", "mouseReleased"] {
            conn.call(
                s,
                "Input.dispatchMouseEvent",
                json!({"type": kind, "x": x, "y": y, "button": b, "clickCount": n, "modifiers": m}),
            )?;
        }
    }
    Ok(())
}

/// A press at `from`, a move in steps, a release at `to`.
pub fn drag(
    conn: &mut Conn,
    page: &Page,
    from: (f64, f64),
    to: (f64, f64),
    cancel: &CancelToken,
) -> CuResult<()> {
    drag_in(conn, &page.session, from, to, cancel)
}

fn drag_in(
    conn: &mut Conn,
    session: &str,
    from: (f64, f64),
    to: (f64, f64),
    cancel: &CancelToken,
) -> CuResult<()> {
    let s = Some(session);
    let ev = |conn: &mut Conn, kind: &str, (x, y): (f64, f64), buttons: u32| {
        conn.call(
            s,
            "Input.dispatchMouseEvent",
            json!({"type": kind, "x": x, "y": y, "button": "left", "buttons": buttons, "clickCount": 1}),
        )
        .map(drop)
    };
    ev(conn, "mouseMoved", from, 0)?;
    ev(conn, "mousePressed", from, 1)?;
    let steps = 10;
    let mut result = Ok(());
    for i in 1..=steps {
        let t = f64::from(i) / f64::from(steps);
        let p = (from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t);
        result = cancel.check().and_then(|()| ev(conn, "mouseMoved", p, 1));
        if result.is_err() {
            break;
        }
        std::thread::sleep(Duration::from_millis(8));
    }
    // The button is let go on every path, as a person's hand would be. When the drag already
    // failed, the release is best effort and the first error is the one reported.
    let released = ev(conn, "mouseReleased", to, 0);
    result.and(released)
}

/// Wheel lines: positive `dy` scrolls towards the end, as elsewhere in the engine.
pub fn wheel(conn: &mut Conn, page: &Page, (x, y): (f64, f64), dx: i32, dy: i32) -> CuResult<()> {
    const LINE: f64 = 40.0;
    conn.call(
        Some(&page.session),
        "Input.dispatchMouseEvent",
        json!({"type": "mouseWheel", "x": x, "y": y, "deltaX": f64::from(dx) * LINE, "deltaY": f64::from(dy) * LINE}),
    )?;
    Ok(())
}

/// Presses and releases one key, with its editing command on macOS (⌘A and the like).
pub fn key(conn: &mut Conn, page: &Page, chord: &Chord) -> CuResult<()> {
    let k: PageKey = page_key(chord)
        .ok_or_else(|| CuError::new(ErrorCode::BadRequest, format!("no key {:?}", chord.key)))?;
    let s = Some(page.session.as_str());
    let m = modifier_bits(chord.mods);
    let mut down = json!({"type": if k.text.is_some() { "keyDown" } else { "rawKeyDown" },
        "key": k.key, "code": k.code, "windowsVirtualKeyCode": k.key_code,
        "nativeVirtualKeyCode": k.key_code, "modifiers": m});
    if let Some(t) = &k.text {
        down["text"] = json!(t);
        down["unmodifiedText"] = json!(t);
    }
    if let Some(c) = mac_command(chord) {
        down["commands"] = json!([c]);
    }
    conn.call(s, "Input.dispatchKeyEvent", down)?;
    conn.call(
        s,
        "Input.dispatchKeyEvent",
        json!({"type": "keyUp", "key": k.key, "code": k.code, "windowsVirtualKeyCode": k.key_code,
               "nativeVirtualKeyCode": k.key_code, "modifiers": m}),
    )?;
    Ok(())
}

/// Types text at the focused element's selection: one insert per line, Return between lines.
pub fn insert(conn: &mut Conn, page: &Page, text: &str, cancel: &CancelToken) -> CuResult<()> {
    let s = Some(page.session.as_str());
    let enter = Chord::parse("return").ok_or_else(|| CuError::new(ErrorCode::Failed, "return"))?;
    for (i, line) in text.split('\n').enumerate() {
        cancel.check()?;
        if i > 0 {
            key(conn, page, &enter)?;
        }
        if !line.is_empty() {
            conn.call(s, "Input.insertText", json!({"text": line}))?;
        }
    }
    Ok(())
}

/// Gives an element the keyboard focus.
pub fn focus(conn: &mut Conn, el: &WebEl) -> CuResult<()> {
    conn.call(
        Some(&el.session),
        "DOM.focus",
        json!({"backendNodeId": el.node}),
    )?;
    Ok(())
}

/// Scrolls an element into view in its own frames.
pub fn scroll_into_view(conn: &mut Conn, el: &WebEl) -> CuResult<()> {
    conn.call(
        Some(&el.session),
        "DOM.scrollIntoViewIfNeeded",
        json!({"backendNodeId": el.node}),
    )?;
    Ok(())
}

/// Whether the element the page would hit at `(x, y)` (main viewport, CSS pixels) is `el` or
/// inside it, frame by frame. Otherwise names what is on top.
pub fn hit(
    conn: &mut Conn,
    page: &Page,
    el: &WebEl,
    (x, y): (f64, f64),
) -> CuResult<Result<(), String>> {
    // The frames from the page down to the element's.
    let chain = page.chain_to(&el.session)?;
    let mut session = page.session.clone();
    let (mut cx, mut cy) = (x, y);
    for (frame, child) in chain {
        let at = node_at(conn, &session, cx, cy)?;
        let owner = conn.call(
            Some(&session),
            "DOM.getFrameOwner",
            json!({"frameId": frame}),
        )?["backendNodeId"]
            .as_i64();
        if at != owner {
            return Ok(Err(describe(conn, &session, at)));
        }
        let q: Rect = content_quad(conn, &session, owner.unwrap_or_default())?;
        cx -= q.x;
        cy -= q.y;
        session = child;
    }
    let at = node_at(conn, &session, cx, cy)?;
    let Some(at_node) = at else {
        return Ok(Err("nothing".into()));
    };
    let inside = contains(conn, el, at_node)?;
    Ok(if inside {
        Ok(())
    } else {
        Err(describe(conn, &session, at))
    })
}

/// The node at a point of a frame's viewport, in CSS pixels. The protocol takes document
/// points, so the frame's scroll is added; a point off the document hits nothing.
fn node_at(conn: &mut Conn, session: &str, x: f64, y: f64) -> CuResult<Option<i64>> {
    let m = conn.call(Some(session), "Page.getLayoutMetrics", json!({}))?;
    let v = &m["cssLayoutViewport"];
    let (sx, sy) = (
        v["pageX"].as_f64().unwrap_or(0.0),
        v["pageY"].as_f64().unwrap_or(0.0),
    );
    let r = conn.call(
        Some(session),
        "DOM.getNodeForLocation",
        json!({"x": (x + sx).round() as i64, "y": (y + sy).round() as i64, "includeUserAgentShadowDOM": true}),
    );
    match r {
        Ok(r) => Ok(r["backendNodeId"].as_i64()),
        Err(e) if e.detail.contains("No node found") => Ok(None),
        Err(e) => Err(e),
    }
}

/// Whether `inner` is `el` or inside it, through shadow roots.
fn contains(conn: &mut Conn, el: &WebEl, inner: i64) -> CuResult<bool> {
    let s = Some(el.session.as_str());
    let resolve = |conn: &mut Conn, node: i64| -> CuResult<String> {
        let r = conn.call(s, "DOM.resolveNode", json!({"backendNodeId": node}))?;
        r["object"]["objectId"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| CuError::new(ErrorCode::StaleRef, "the element is gone"))
    };
    let outer = resolve(conn, el.node)?;
    let inner = resolve(conn, inner)?;
    let r = conn.call(
        s,
        "Runtime.callFunctionOn",
        json!({"objectId": outer,
               "functionDeclaration": "function(n){for(let x=n;x;x=x.parentNode||x.host){if(x===this)return true}return false}",
               "arguments": [{"objectId": inner}], "returnByValue": true}),
    )?;
    for o in [outer, inner] {
        let _ = conn.call(s, "Runtime.releaseObject", json!({"objectId": o}));
    }
    Ok(r["result"]["value"].as_bool().unwrap_or(false))
}

/// `<div id="x">` for an element, for error messages.
fn describe(conn: &mut Conn, session: &str, node: Option<i64>) -> String {
    let Some(node) = node else {
        return "nothing".into();
    };
    let Ok(r) = conn.call(
        Some(session),
        "DOM.describeNode",
        json!({"backendNodeId": node}),
    ) else {
        return "another element".into();
    };
    let n = &r["node"];
    let name = n["localName"].as_str().unwrap_or("element");
    let id = n["attributes"].as_array().and_then(|a| {
        a.chunks(2)
            .find(|kv| kv[0] == "id")
            .and_then(|kv| kv.get(1)?.as_str())
    });
    match id {
        Some(id) => format!("<{name} id=\"{id}\">"),
        None => format!("<{name}>"),
    }
}

/// Replaces an editable element's whole text: selects it all, then inserts over it (or deletes
/// it, for empty text), so the page sees trusted input as from a person.
pub fn replace_text(
    conn: &mut Conn,
    page: &Page,
    el: &WebEl,
    text: &str,
    cancel: &CancelToken,
) -> CuResult<()> {
    focus(conn, el)?;
    call_on(
        conn,
        el,
        "function(){ if (typeof this.select === 'function') { this.select(); return; } \
         const r = document.createRange(); r.selectNodeContents(this); \
         const s = getSelection(); s.removeAllRanges(); s.addRange(r); }",
        &[],
    )?;
    if text.is_empty() {
        let del = Chord::parse("backspace")
            .ok_or_else(|| CuError::new(ErrorCode::Failed, "backspace"))?;
        return key(conn, page, &del);
    }
    insert(conn, page, text, cancel)
}

/// The element's text as the page holds it: a field's value, else its text.
pub fn text_of(conn: &mut Conn, el: &WebEl) -> CuResult<String> {
    let v = call_on(
        conn,
        el,
        "function(){ return this.type === 'password' ? null : ('value' in this ? String(this.value) : this.textContent); }",
        &[],
    )?;
    match v {
        Value::String(s) => Ok(s),
        Value::Null => err(ErrorCode::SecureField, "that is a password field"),
        other => Ok(other.to_string()),
    }
}

/// The `<select>` an `<option>` is in, and the option's place in its list. A closed list's
/// options have no box to click (the protocol answered "Node does not have a layout object"), so
/// a click on one picks it in its list. `None` for any other element.
pub fn option_list(conn: &mut Conn, el: &WebEl) -> CuResult<Option<(WebEl, usize)>> {
    let index = call_on(
        conn,
        el,
        "function(){ return this.tagName === 'OPTION' && this.closest('select') ? this.index : null; }",
        &[],
    )?;
    let Some(index) = index.as_u64().map(|i| i as usize) else {
        return Ok(None);
    };
    let s = Some(el.session.as_str());
    let gone = || CuError::new(ErrorCode::StaleRef, "the element is gone");
    let r = conn.call(s, "DOM.resolveNode", json!({"backendNodeId": el.node}))?;
    let option = r["object"]["objectId"]
        .as_str()
        .ok_or_else(gone)?
        .to_owned();
    let list = conn.call(
        s,
        "Runtime.callFunctionOn",
        json!({"objectId": option, "functionDeclaration": "function(){ return this.closest('select'); }"}),
    );
    let _ = conn.call(s, "Runtime.releaseObject", json!({"objectId": option}));
    let list = list?["result"]["objectId"]
        .as_str()
        .ok_or_else(gone)?
        .to_owned();
    let node = conn.call(s, "DOM.describeNode", json!({"objectId": list}));
    let _ = conn.call(s, "Runtime.releaseObject", json!({"objectId": list}));
    let node = node?["node"]["backendNodeId"].as_i64().ok_or_else(gone)?;
    Ok(Some((
        WebEl {
            session: el.session.clone(),
            node,
        },
        index,
    )))
}

/// The option to pick: one named by its label or value, or the very one a worker clicked (two
/// options may share a label).
pub enum Choice<'a> {
    Named(&'a str),
    At(usize),
}

/// How long a list's type-ahead keeps what was typed: Chromium's is a second.
const TYPE_AHEAD_FORGETS: std::time::Duration = std::time::Duration::from_millis(1100);

/// Picks a `<select>`'s option with the keyboard, as a person would without opening its menu:
/// focus, then type the option's label. Returns the option's place in its list.
pub fn pick_option(
    conn: &mut Conn,
    page: &Page,
    el: &WebEl,
    want: Choice<'_>,
    cancel: &CancelToken,
) -> CuResult<usize> {
    let options = call_on(
        conn,
        el,
        "function(){ return Array.from(this.options).map(o => [o.label, o.value]); }",
        &[],
    )?;
    let options: Vec<(String, String)> = options
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|o| Some((o[0].as_str()?.to_owned(), o[1].as_str()?.to_owned())))
        .collect();
    let index = match want {
        Choice::At(i) if i < options.len() => i,
        Choice::At(_) => return err(ErrorCode::StaleRef, "the option is gone"),
        Choice::Named(want) => options
            .iter()
            .position(|(l, v)| l == want || v == want)
            .or_else(|| {
                options
                    .iter()
                    .position(|(l, _)| l.eq_ignore_ascii_case(want.trim()))
            })
            .ok_or_else(|| {
                CuError::new(
                    ErrorCode::NotSettable,
                    format!(
                        "no option {want:?}; it has {}",
                        options
                            .iter()
                            .map(|(l, _)| format!("{l:?}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                )
            })?,
    };
    let label = options[index].0.clone();
    let selected = |conn: &mut Conn| -> CuResult<i64> {
        Ok(
            call_on(conn, el, "function(){ return this.selectedIndex; }", &[])?
                .as_i64()
                .unwrap_or(-1),
        )
    };
    if selected(conn)? == index as i64 {
        return Ok(index);
    }
    let chord_of = |c: char| Chord {
        mods: Mods {
            shift: c.is_uppercase(),
            ..Mods::default()
        },
        key: c.to_lowercase().collect(),
    };
    focus(conn, el)?;
    // Typing a label selects the first option it begins; the keys are the page's own input.
    for c in label.chars() {
        cancel.check()?;
        if c == ' ' {
            key(conn, page, &Chord::parse("space").unwrap_or(chord_of(c)))?;
        } else {
            key(conn, page, &chord_of(c))?;
        }
    }
    if selected(conn)? == index as i64 {
        return Ok(index);
    }
    // Typing stops at the first option a label begins (two may share it). An arrow key would
    // open a Mac list's menu rather than step. Once the list forgets what was typed, its first
    // letter again steps to the next option it begins, round the list.
    if let Some(first) = label.chars().next() {
        let until = std::time::Instant::now() + TYPE_AHEAD_FORGETS;
        while std::time::Instant::now() < until {
            cancel.check()?;
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        for _ in 0..options.len() {
            cancel.check()?;
            key(conn, page, &chord_of(first))?;
            if selected(conn)? == index as i64 {
                return Ok(index);
            }
        }
    }
    err(
        ErrorCode::NotSettable,
        format!("the list didn't take {label:?}; click the menu and pick it instead"),
    )
}

/// Sets the selected text range of a field, counted in characters.
pub fn select_range(conn: &mut Conn, el: &WebEl, start: usize, length: usize) -> CuResult<()> {
    focus(conn, el)?;
    let ok = call_on(
        conn,
        el,
        "function(s, n){ if (typeof this.setSelectionRange !== 'function') return false; \
         const chars = Array.from(this.value); \
         const a = chars.slice(0, s).join('').length, b = chars.slice(0, s + n).join('').length; \
         this.setSelectionRange(a, b); return true; }",
        &[json!(start), json!(length)],
    )?;
    if ok.as_bool() != Some(true) {
        return err(
            ErrorCode::NotSettable,
            "that element has no text range to select",
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;
    use std::thread;

    use tungstenite::Message;

    use super::*;
    use crate::cancel::Generations;

    /// A browser endpoint on a local port that answers every command, except that it fails
    /// mouse moves made with the button down. It returns the commands it was sent.
    fn browser_failing_held_moves() -> (u16, thread::JoinHandle<Vec<Value>>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut ws = tungstenite::accept(stream).unwrap();
            let mut sent = Vec::new();
            while let Ok(msg) = ws.read() {
                let Message::Text(text) = msg else { continue };
                let cmd: Value = serde_json::from_str(&text).unwrap();
                let p = &cmd["params"];
                let reply = if p["type"] == "mouseMoved" && p["buttons"] == 1 {
                    json!({"id": cmd["id"], "error": {"message": "the target closed"}})
                } else {
                    json!({"id": cmd["id"], "result": {}})
                };
                sent.push(cmd);
                ws.send(Message::text(reply.to_string())).unwrap();
            }
            sent
        });
        (port, server)
    }

    #[test]
    fn a_drag_whose_move_fails_still_lets_the_button_go_and_reports_the_move() {
        let (port, server) = browser_failing_held_moves();
        let mut conn = Conn::connect(port, "/").unwrap();
        let cancel = Generations::new().token("s", Duration::from_secs(60));
        let e = drag_in(&mut conn, "page", (10.0, 10.0), (50.0, 50.0), &cancel).unwrap_err();
        assert_eq!(e.code, ErrorCode::Failed);
        assert!(e.detail.contains("the target closed"), "{}", e.detail);
        drop(conn);
        let kinds: Vec<String> = server
            .join()
            .unwrap()
            .iter()
            .map(|c| c["params"]["type"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(
            kinds,
            ["mouseMoved", "mousePressed", "mouseMoved", "mouseReleased"]
        );
    }
}
