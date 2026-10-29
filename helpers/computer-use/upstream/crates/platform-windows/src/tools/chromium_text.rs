//! Caret insertion into an exact Chromium page field, without native focus.
//!
//! UIA SetFocus requests application focus, and Chromium can stop accepting
//! WM_CHAR as soon as hand-back blurs its input client. Never send that prefix
//! and then retry another route. Use an owned, already enabled CDP endpoint or
//! refuse before input. ValuePattern is read-only here; Input.insertText edits
//! the existing selection rather than replacing a document or field value.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use cua_driver_core::action_record::{
    ActionEffect, ActionEvidence, ActionExecutionRecord, ActionTransport, ActualDelivery,
    EvidenceKind, RequestedDelivery,
};
use cua_driver_core::browser::binding::{correlate, BindingOutcome, CdpWindowCandidate};
use cua_driver_core::browser::cdp_ws::{CdpConnection, CdpPool};
use cua_driver_core::browser::types::{BindingQuality, NativeWindowInfo, Rect};
use cua_driver_core::browser::BrowserPlatform;
use cua_driver_core::protocol::ToolResult;
use serde_json::{json, Value};
use windows::core::Interface;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::UI::Accessibility::*;

use crate::browser_platform::WindowsBrowserPlatform;
use crate::uia::snapshot::RetainedElement;

static GATE: OnceLock<Arc<tokio::sync::Mutex<()>>> = OnceLock::new();
static GROUP: AtomicU64 = AtomicU64::new(0);

struct SessionState {
    conn: Arc<CdpConnection>,
    session: String,
    group: String,
    emulated: bool,
    // Keep serialization through cancellation cleanup, so it cannot disable
    // emulation belonging to the next insertion in this helper.
    _serial: Option<tokio::sync::OwnedMutexGuard<()>>,
}

struct SessionCleanup(Option<SessionState>);

async fn clean_session(state: &SessionState) -> Result<()> {
    let mut commands = Vec::new();
    if state.emulated {
        commands.push((
            Some(state.session.as_str()),
            "Emulation.setFocusEmulationEnabled",
            json!({"enabled":false}),
        ));
    }
    commands.push((
        Some(state.session.as_str()),
        "Runtime.releaseObjectGroup",
        json!({"objectGroup":state.group}),
    ));
    commands.push((
        None,
        "Target.detachFromTarget",
        json!({"sessionId":state.session}),
    ));
    let mut errors = Vec::new();
    for (session, method, arguments) in commands {
        match tokio::time::timeout(
            Duration::from_secs(1),
            state.conn.call(session, method, arguments),
        )
        .await
        {
            Ok(Ok(_)) => {}
            other => errors.push(format!("{method}: {other:?}")),
        }
    }
    if !errors.is_empty() {
        bail!("page-session cleanup is unconfirmed: {}", errors.join("; "));
    }
    Ok(())
}

impl SessionCleanup {
    async fn finish(&mut self) -> Result<()> {
        let result = clean_session(self.0.as_ref().unwrap()).await;
        self.0.take();
        result
    }
}

impl Drop for SessionCleanup {
    fn drop(&mut self) {
        if let Some(state) = self.0.take() {
            // Also runs if the caller's deadline cancels the in-flight future.
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    if let Err(error) = clean_session(&state).await {
                        tracing::warn!(%error, "cancelled background text session cleanup failed");
                    }
                });
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Field {
    id: String,
    name: String,
    url: String,
    value: String,
    prefix: String,
    selected: String,
    suffix: String,
    selection_from_uia: bool,
}

impl Field {
    fn selection(&self) -> (usize, usize) {
        let start = self.prefix.encode_utf16().count();
        (start, start + self.selected.encode_utf16().count())
    }

    fn expected(&self, text: &str) -> (String, usize) {
        (
            format!("{}{text}{}", self.prefix, self.suffix),
            self.prefix.encode_utf16().count() + text.encode_utf16().count(),
        )
    }
}

/// No UIA mutation or system-wide focused-element query is used here.
unsafe fn read_field(hwnd: u64, retained: &RetainedElement) -> Result<Field> {
    if !retained.is_uia() {
        bail!("the exact page field needs a UI Automation element token");
    }
    let element = std::mem::ManuallyDrop::new(unsafe {
        IUIAutomationElement::from_raw(retained.as_ptr() as *mut _)
    });
    let kind = unsafe { element.CurrentControlType()? };
    if kind != UIA_EditControlTypeId && kind != UIA_ComboBoxControlTypeId {
        bail!("this Chromium element is not an editable input field");
    }
    if unsafe { element.CurrentIsOffscreen()? }.as_bool() {
        bail!("the addressed field is not exposed in the visible tab");
    }
    let vp: IUIAutomationValuePattern =
        unsafe { element.GetCurrentPattern(UIA_ValuePatternId)?.cast()? };
    if unsafe { vp.CurrentIsReadOnly()? }.as_bool() {
        bail!("the addressed field is read-only");
    }
    let value = unsafe { vp.CurrentValue()? }.to_string();
    // A Chromium ComboBox TextPattern may represent its embedded edit as
    // U+FFFC, rather than the field value. Do not interpret that as a caret,
    // strip meaningful text, or guess append. The exact DOM field can supply
    // its own selection after its identity and full value have been proven.
    let selection: Result<_> = (|| unsafe {
        let tp: IUIAutomationTextPattern = element.GetCurrentPattern(UIA_TextPatternId)?.cast()?;
        let ranges = tp.GetSelection()?;
        if ranges.Length()? != 1 {
            bail!("not one UIA selection");
        }
        let selected = ranges.GetElement(0)?;
        let document = tp.DocumentRange()?;
        let prefix = document.Clone()?;
        let suffix = document.Clone()?;
        prefix.MoveEndpointByRange(
            TextPatternRangeEndpoint_End,
            &selected,
            TextPatternRangeEndpoint_Start,
        )?;
        suffix.MoveEndpointByRange(
            TextPatternRangeEndpoint_Start,
            &selected,
            TextPatternRangeEndpoint_End,
        )?;
        let prefix = prefix.GetText(-1)?.to_string();
        let selected = selected.GetText(-1)?.to_string();
        let suffix = suffix.GetText(-1)?.to_string();
        if format!("{prefix}{selected}{suffix}") != value
            || document.GetText(-1)?.to_string() != value
        {
            bail!("UIA TextPattern does not describe this field value");
        }
        Ok((prefix, selected, suffix))
    })();
    let selection_from_uia = selection.is_ok();
    let (prefix, selected, suffix) =
        selection.unwrap_or_else(|_| (String::new(), String::new(), value.clone()));
    let automation: IUIAutomation =
        unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)? };
    let root = unsafe { automation.ElementFromHandle(HWND(hwnd as *mut _))? };
    let walker = unsafe { automation.ControlViewWalker()? };
    let mut parent = unsafe { walker.GetParentElement(&*element)? };
    let mut url = None;
    let mut page = None;
    let mut belongs = false;
    for _ in 0..128 {
        if unsafe { automation.CompareElements(&parent, &root)? }.as_bool() {
            belongs = true;
            break;
        }
        if url.is_none() && unsafe { parent.CurrentControlType()? } == UIA_DocumentControlTypeId {
            url = unsafe { super::launch_browser::document_url(&parent) };
            if url.is_some() {
                page = Some(parent.clone());
            }
        }
        parent = unsafe { walker.GetParentElement(&parent)? };
    }
    if !belongs {
        bail!("the field no longer belongs to the addressed window");
    }
    let url = url.context("the field is not inside a URL-identified page")?;
    let (_, visible, _) = unsafe { super::launch_browser::matching_document(hwnd, &url, true)? };
    if !unsafe { automation.CompareElements(&visible, &page.unwrap())? }.as_bool() {
        bail!("the addressed field is not in the visible page");
    }
    Ok(Field {
        id: unsafe { element.CurrentAutomationId()? }.to_string(),
        name: unsafe { element.CurrentName()? }.to_string(),
        url,
        value,
        prefix,
        selected,
        suffix,
        selection_from_uia,
    })
}

async fn field_snapshot(hwnd: u64, retained: RetainedElement) -> Result<Field> {
    tokio::task::spawn_blocking(move || {
        crate::uia::windows_enum::run_uia_with_deadline_cancelable(
            "Chromium field selection",
            Duration::from_secs(2),
            "no text input",
            move |_| {
                let _com = super::launch_browser::Apartment::new();
                unsafe { read_field(hwnd, &retained) }
            },
        )
        .map_err(|error| {
            anyhow!(
                "field selection read stopped: {}",
                crate::uia::click::deadline_reason(error)
            )
        })?
    })
    .await?
}

fn same_url(a: &str, b: &str) -> bool {
    fn root(url: &str) -> bool {
        url.split_once("://")
            .is_some_and(|(_, host)| !host.is_empty() && !host.contains(['/', '?', '#']))
    }
    a == b
        || a.strip_suffix('/').is_some_and(|a| root(a) && a == b)
        || b.strip_suffix('/').is_some_and(|b| root(b) && b == a)
}

async fn bound_target(
    conn: &CdpConnection,
    native: &NativeWindowInfo,
    url: &str,
) -> Result<String> {
    let targets = conn.call(None, "Target.getTargets", json!({})).await?;
    let targets = targets["targetInfos"]
        .as_array()
        .context("missing browser targets")?;
    if targets.len() > 128 {
        bail!("too many targets to prove an exact page within this action");
    }
    let mut candidates = Vec::new();
    for tab in targets.iter().filter(|tab| tab["type"] == "page") {
        let id = tab["targetId"]
            .as_str()
            .context("missing browser target id")?;
        let window = conn
            .call(None, "Browser.getWindowForTarget", json!({"targetId":id}))
            .await?;
        let b = &window["bounds"];
        candidates.push(CdpWindowCandidate {
            cdp_target_id: id.to_owned(),
            cdp_window_id: Some(
                window["windowId"]
                    .as_i64()
                    .context("missing browser window id")?,
            ),
            bounds: Some(Rect::new(
                b["left"].as_f64().context("missing browser left")?,
                b["top"].as_f64().context("missing browser top")?,
                b["width"].as_f64().context("missing browser width")?,
                b["height"].as_f64().context("missing browser height")?,
            )),
            title: tab["title"].as_str().unwrap_or_default().into(),
            url: tab["url"].as_str().unwrap_or_default().into(),
        });
    }
    select_bound_target(native, &candidates, url)
}

fn select_bound_target(
    native: &NativeWindowInfo,
    candidates: &[CdpWindowCandidate],
    url: &str,
) -> Result<String> {
    let BindingOutcome::Bound {
        candidate,
        quality: BindingQuality::Exact,
    } = correlate(native, candidates, 2.0)
    else {
        bail!(
            "the DevTools browser window cannot be bound uniquely to the addressed Windows window"
        );
    };
    let mut visible = candidates
        .iter()
        .filter(|tab| tab.cdp_window_id == candidate.cdp_window_id && same_url(&tab.url, url));
    let tab = visible
        .next()
        .context("the visible UIA page URL is absent from the bound browser window")?;
    if visible.next().is_some() {
        bail!("more than one tab has the visible page URL; no tab was guessed");
    }
    Ok(tab.cdp_target_id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_driver_core::browser::types::{NativeOwnershipMethod, NativeOwnershipProof};

    fn field(prefix: &str, selected: &str, suffix: &str) -> Field {
        Field {
            id: "field".into(),
            name: "Fixture".into(),
            url: "https://fixture.test/".into(),
            value: format!("{prefix}{selected}{suffix}"),
            prefix: prefix.into(),
            selected: selected.into(),
            suffix: suffix.into(),
            selection_from_uia: true,
        }
    }

    #[test]
    fn unicode_caret_insertion_preserves_both_sides_and_replaces_only_selection() {
        let f = field("A😀", "SELECT", " suffix");
        assert_eq!(f.selection(), (3, 9));
        assert_eq!(f.expected("é🦀"), ("A😀é🦀 suffix".into(), 6));
        let after =
            json!({"value":"A😀é🦀 suffix","start":6,"end":6,"url":"https://fixture.test/"});
        assert!(matches_after(&after, &f, "é🦀"));
    }

    #[test]
    fn complete_readback_rejects_prefix_only_wrong_caret_and_lost_existing_text() {
        let f = field("old ", "", " tail");
        assert!(!matches_after(
            &json!({"value":"old H tail","start":5,"end":5,"url":f.url}),
            &f,
            "Hello"
        ));
        assert!(!matches_after(
            &json!({"value":"old Hello tail","start":0,"end":0,"url":f.url}),
            &f,
            "Hello"
        ));
        assert!(!matches_after(
            &json!({"value":"Hello","start":5,"end":5,"url":f.url}),
            &f,
            "Hello"
        ));
        assert!(matches_after(
            &json!({"value":"old Hello tail","start":9,"end":9,"url":f.url}),
            &f,
            "Hello"
        ));
    }

    #[test]
    fn stale_selection_or_value_refuses_before_insertion() {
        let f = field("before ", "selected", " after");
        let mut state = json!({"value":f.value,"start":7,"end":15,"url":f.url});
        assert!(matches_before(&state, &f));
        state["start"] = 0.into();
        assert!(!matches_before(&state, &f));
        state["start"] = 7.into();
        state["value"] = "other value".into();
        assert!(!matches_before(&state, &f));
    }

    #[test]
    fn page_identity_does_not_conflate_path_slashes_query_or_fragment() {
        assert!(same_url("https://fixture.test/", "https://fixture.test"));
        assert!(!same_url(
            "https://fixture.test/path/",
            "https://fixture.test/path"
        ));
        assert!(!same_url(
            "https://fixture.test/?q=a",
            "https://fixture.test/?q=b"
        ));
        assert!(!same_url(
            "https://fixture.test/#a",
            "https://fixture.test/#b"
        ));
    }

    #[test]
    fn duplicate_matching_tabs_and_heuristic_only_windows_are_refused() {
        let bounds = Rect::new(100.0, 50.0, 800.0, 600.0);
        let native = NativeWindowInfo {
            pid: 42,
            window_id: 1,
            title: "Fixture - Chrome".into(),
            bounds,
            geometry_exact: true,
            ownership: NativeOwnershipProof {
                method: NativeOwnershipMethod::WindowServerOwner,
                owner_pid: 42,
                detail: None,
            },
        };
        let tab = CdpWindowCandidate {
            cdp_target_id: "one".into(),
            cdp_window_id: Some(7),
            title: "Fixture".into(),
            url: "https://fixture.test/".into(),
            bounds: Some(bounds),
        };
        assert_eq!(
            select_bound_target(&native, std::slice::from_ref(&tab), "https://fixture.test/")
                .unwrap(),
            "one"
        );
        let mut second = tab.clone();
        second.cdp_target_id = "two".into();
        assert!(
            select_bound_target(&native, &[tab.clone(), second], "https://fixture.test/").is_err()
        );
        let mut heuristic = tab;
        heuristic.bounds = None;
        assert!(select_bound_target(&native, &[heuristic], "https://fixture.test/").is_err());
    }

    #[tokio::test]
    async fn empty_input_never_enters_a_desktop_route_or_deletes_selection() {
        let result = insert(0, 0, None, "").await;
        assert_ne!(result.is_error, Some(true));
        assert_eq!(result.structured_content.unwrap()["characters"], 0);
    }

    #[test]
    fn confirmed_browser_result_includes_the_required_execution_record() {
        let result = confirmed_result(8);
        let record = result
            .action_record
            .as_ref()
            .expect("never omit the internal outcome record");
        assert!(record.validate().is_ok());
        assert_eq!(
            result.structured_content.unwrap()["verify"],
            "complete_value_and_caret"
        );
    }

    #[test]
    fn embedded_edit_uses_exact_dom_selection_without_guessing_append() {
        let mut wrapper = field("", "", "A😀 tail");
        wrapper.selection_from_uia = false; // ComboBox TextPattern was U+FFFC.
        let dom = json!({"value":"A😀 tail","start":1,"end":3,"url":wrapper.url});
        let planned = plan_from_dom(&wrapper, &dom).unwrap();
        assert_eq!(planned.expected("É"), ("AÉ tail".into(), 2));
        assert!(same_visible_field(&wrapper, &planned));
        let split = json!({"value":"A😀 tail","start":2,"end":3,"url":wrapper.url});
        assert!(plan_from_dom(&wrapper, &split).is_err());
        let mut changed = wrapper.clone();
        changed.value = "user changed it".into();
        assert!(!same_visible_field(&changed, &planned));
    }

    #[test]
    fn known_uia_selection_never_yields_to_a_different_dom_selection() {
        let f = field("prefix ", "selected", " suffix");
        let dom = json!({"value":f.value,"start":0,"end":0,"url":f.url});
        assert!(plan_from_dom(&f, &dom).is_err());
    }
}

const FIND_FIELD: &str = r#"function(meta) {
    const editable = e => (e instanceof HTMLInputElement || e instanceof HTMLTextAreaElement)
        && !e.disabled && !e.readOnly && e.type !== 'password'
        && Number.isInteger(e.selectionStart) && Number.isInteger(e.selectionEnd);
    let nodes;
    if (meta.id) nodes = [...document.querySelectorAll('#' + CSS.escape(meta.id))];
    else {
        const all = document.querySelectorAll('input,textarea');
        if (all.length > 2048 || !meta.name) return null;
        nodes = [...all].filter(e => {
            const names = [e.getAttribute('aria-label'), e.placeholder, e.title,
                ...[...(e.labels || [])].map(l => l.textContent.trim())];
            const labelled = e.getAttribute('aria-labelledby');
            if (labelled) names.push(labelled.split(/\s+/).map(id => document.getElementById(id)?.textContent || '').join(' ').trim());
            return names.includes(meta.name);
        });
    }
    return nodes.length === 1 && editable(nodes[0]) ? nodes[0] : null;
}"#;

const FIELD_STATE: &str = r#"function(selection) {
    if (!this.isConnected || this.ownerDocument !== document || this.type === 'password') return null;
    if (selection) {
        if (this.value !== selection.value) return null;
        this.setSelectionRange(selection.start, selection.end, selection.direction);
    }
    return {value:this.value, start:this.selectionStart, end:this.selectionEnd,
        direction:this.selectionDirection, url:document.location.href,
        focused:document.activeElement === this && document.hasFocus()};
}"#;

async fn state(
    conn: &CdpConnection,
    session: &str,
    object: &str,
    selection: Option<Value>,
) -> Result<Value> {
    let arguments = selection
        .map(|value| vec![json!({"value":value})])
        .unwrap_or_default();
    let result = conn.call(Some(session), "Runtime.callFunctionOn", json!({
        "objectId":object,"functionDeclaration":FIELD_STATE,"arguments":arguments,"returnByValue":true,
    })).await?;
    if result.get("exceptionDetails").is_some() {
        bail!("the page field state could not be read");
    }
    Ok(result["result"]["value"].clone())
}

fn matches_before(state: &Value, field: &Field) -> bool {
    let (start, end) = field.selection();
    state["value"].as_str() == Some(field.value.as_str())
        && state["start"].as_u64() == Some(start as u64)
        && state["end"].as_u64() == Some(end as u64)
        && state["url"]
            .as_str()
            .is_some_and(|url| same_url(url, &field.url))
}

fn plan_from_dom(field: &Field, state: &Value) -> Result<Field> {
    if state["value"].as_str() != Some(field.value.as_str())
        || !state["url"]
            .as_str()
            .is_some_and(|url| same_url(url, &field.url))
    {
        bail!("the exact DOM field value/page does not agree with UIA");
    }
    let start = state["start"]
        .as_u64()
        .and_then(|v| usize::try_from(v).ok())
        .context("the DOM field does not expose a selection start")?;
    let end = state["end"]
        .as_u64()
        .and_then(|v| usize::try_from(v).ok())
        .context("the DOM field does not expose a selection end")?;
    let units: Vec<_> = field.value.encode_utf16().collect();
    if start > end || end > units.len() {
        bail!("the DOM selection is outside the field value");
    }
    if field.selection_from_uia && field.selection() != (start, end) {
        bail!("UIA and DOM selections disagree");
    }
    let mut planned = field.clone();
    planned.prefix =
        String::from_utf16(&units[..start]).context("selection splits a Unicode character")?;
    planned.selected =
        String::from_utf16(&units[start..end]).context("selection splits a Unicode character")?;
    planned.suffix =
        String::from_utf16(&units[end..]).context("selection splits a Unicode character")?;
    Ok(planned)
}

fn same_visible_field(current: &Field, planned: &Field) -> bool {
    current.id == planned.id
        && current.name == planned.name
        && current.url == planned.url
        && current.value == planned.value
        && (!current.selection_from_uia || current.selection() == planned.selection())
}

fn matches_after(state: &Value, field: &Field, text: &str) -> bool {
    let (expected, caret) = field.expected(text);
    state["value"].as_str() == Some(expected.as_str())
        && state["start"].as_u64() == Some(caret as u64)
        && state["end"].as_u64() == Some(caret as u64)
        && state["url"]
            .as_str()
            .is_some_and(|url| same_url(url, &field.url))
}

fn confirmed_result(count: usize) -> ToolResult {
    let record = ActionExecutionRecord::builder(
        ActionEffect::Confirmed, ActionTransport::BrowserCdpInputKey, RequestedDelivery::Background,
    )
    .actual_delivery(ActualDelivery::Background)
    .delivered_count(count as u32)
    .evidence(ActionEvidence {
        kind: EvidenceKind::BrowserReadback,
        detail: "complete field value and collapsed caret match the original selection insertion, after focus emulation ended".into(),
    }).build().expect("complete browser value/caret evidence is a valid confirmed action record");
    ToolResult::text(format!("Inserted {count} character(s) into the exact background page field; confirmed the complete value and caret."))
        .with_structured(json!({"route":"cdp_caret_insertion","effect":"confirmed","verified":true,"verify":"complete_value_and_caret","characters":count}))
        .with_action_record(record)
}

/// Never falls through to native focus or character dispatch, including after
/// an ambiguous CDP response. Once Input.insertText is attempted it may have
/// edited the field even when the connection or the final readback fails.
pub(super) async fn insert(
    pid: u32,
    hwnd: u64,
    retained: Option<RetainedElement>,
    text: &str,
) -> ToolResult {
    if text.is_empty() {
        let record = ActionExecutionRecord::builder(
            ActionEffect::SuspectedNoop,
            ActionTransport::BrowserCdpInputKey,
            RequestedDelivery::Background,
        )
        .actual_delivery(ActualDelivery::NotApplicable)
        .delivered_count(0)
        .detail("empty input returned before any desktop or browser operation")
        .build()
        .expect("no-input result has explicit evidence");
        return ToolResult::text(
            "No text was requested; the field and its selection were left unchanged.",
        )
        .with_structured(json!({"characters":0}))
        .with_action_record(record);
    }
    let mut serial = Some(
        GATE.get_or_init(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
            .lock_owned()
            .await,
    );
    let _guard = crate::input::NoActivateGuard::arm(HWND(hwnd as *mut _));
    let generation = crate::input::inject::foreground_generation();
    let mut attempted = false;
    let result = tokio::time::timeout(Duration::from_secs(20), async {
        let retained = retained.context("Chromium background insertion requires an exact field element_token")?;
        let field = field_snapshot(hwnd, retained.clone()).await?;
        let platform = WindowsBrowserPlatform::default();
        let fingerprint = platform.process_fingerprint(pid.into()).await.map_err(|e| anyhow!(e.message))?;
        let native = platform.native_window(pid.into(), hwnd).await.map_err(|e| anyhow!(e.message))?;
        let endpoint = platform.discover_owned_endpoint(pid.into()).await.map_err(|e| anyhow!(e.message))?
            .context("this browser has no uniquely owned DevTools endpoint; the native Chromium text path would require application focus")?;
        static POOL: OnceLock<CdpPool> = OnceLock::new();
        let conn = POOL.get_or_init(CdpPool::new).get(&endpoint.ws_url).await?;
        let target = bound_target(&conn, &native, &field.url).await?;
        let attached = conn.call(None, "Target.attachToTarget", json!({"targetId":target,"flatten":true})).await?;
        let session = attached["sessionId"].as_str().context("missing exact page session")?;
        let group = format!("brigadier-text-{}-{}", std::process::id(), GROUP.fetch_add(1, Ordering::Relaxed));
        let mut cleanup = SessionCleanup(Some(SessionState {
            conn: conn.clone(), session: session.into(), group: group.clone(),
            emulated: false, _serial: serial.take(),
        }));
        let operation: Result<()> = async {
            let expression = format!("({FIND_FIELD})({})", json!({"id":field.id,"name":field.name}));
            let found = conn.call(Some(session), "Runtime.evaluate", json!({
                "expression":expression,"objectGroup":group,"returnByValue":false,"userGesture":false,
            })).await?;
            let object = found["result"]["objectId"].as_str().context("the UIA field does not match one editable DOM input")?;
            let before = state(&conn, session, object, None).await?;
            let field = plan_from_dom(&field, &before)?;
            if crate::input::inject::foreground_generation() != generation { bail!("physical input interrupted background insertion"); }
            cleanup.0.as_mut().unwrap().emulated = true;
            conn.call(Some(session), "Emulation.setFocusEmulationEnabled", json!({"enabled":true})).await?;
            if crate::input::inject::foreground_generation() != generation { bail!("physical input interrupted background insertion"); }
            conn.call(Some(session), "DOM.focus", json!({"objectId":object})).await?;
            let (start, end) = field.selection();
            let selection = json!({"value":field.value,"start":start,"end":end,"direction":before["direction"]});
            let mut ready = false;
            for _ in 0..10 {
                if crate::input::inject::foreground_generation() != generation { bail!("physical input interrupted background insertion"); }
                let current = state(&conn, session, object, Some(selection.clone())).await?;
                if matches_before(&current, &field) && current["focused"] == true { ready = true; break; }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            if !ready { bail!("the page did not retain its emulated field focus and exact selection"); }
            if !same_visible_field(&field_snapshot(hwnd, retained).await?, &field) { bail!("the addressed visible UIA field changed before insertion"); }
            let current_native = platform.native_window(pid.into(), hwnd).await.map_err(|e| anyhow!(e.message))?;
            let current_fingerprint = platform.process_fingerprint(pid.into()).await.map_err(|e| anyhow!(e.message))?;
            if current_fingerprint != fingerprint || current_native.title != native.title
                || !current_native.bounds.approx_eq(&native.bounds, 0.0)
                || crate::input::inject::foreground_generation() != generation
            { bail!("the browser identity/window or physical input changed before insertion"); }
            let current = state(&conn, session, object, None).await?;
            if !matches_before(&current, &field) || current["focused"] != true { bail!("the exact field or selection changed before insertion"); }
            attempted = true;
            conn.call(Some(session), "Input.insertText", json!({"text":text})).await?;
            let mut landed = false;
            for _ in 0..20 {
                if crate::input::inject::foreground_generation() != generation { bail!("physical input occurred after dispatch; read fresh state"); }
                if matches_after(&state(&conn, session, object, None).await?, &field, text) { landed = true; break; }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            if !landed { bail!("the complete field value and resulting caret were not confirmed"); }
            // Check the final value/caret after ending emulation as well. Blur
            // handlers must not silently invalidate the evidence we return.
            conn.call(Some(session), "Emulation.setFocusEmulationEnabled", json!({"enabled":false})).await?;
            cleanup.0.as_mut().unwrap().emulated = false;
            if crate::input::inject::foreground_generation() != generation
                || !matches_after(&state(&conn, session, object, None).await?, &field, text)
            { bail!("the field or physical input changed while ending emulated focus"); }
            Ok(())
        }.await;
        let cleaned = cleanup.finish().await;
        operation.and(cleaned)
    }).await;
    match result {
        Ok(Ok(())) => confirmed_result(text.chars().count()),
        failure => {
            let why = match failure {
                Ok(Err(e)) => e.to_string(),
                _ => "the bounded insertion deadline elapsed".into(),
            };
            if attempted {
                ToolResult::error(format!("Background text insertion is unconfirmed: {why}. Some text may already have been inserted. Read fresh state before retrying; no alternate route was attempted."))
            } else {
                ToolResult::error(format!("background_unavailable: {why}. No text was sent, no native focus was requested, and no whole-value replacement was attempted. This route needs an already-enabled, proven DevTools endpoint. It will not enable debugging on an existing profile or launch another browser in place of the addressed field."))
            }
        }
    }
}

#[cfg(test)]
mod live_fixture {
    /// Explicit manual fixture, never part of unattended tests. Starts only a
    /// protected Chrome installation with a brand-new direct child of the
    /// TEMP test root. The process is suspended until its exact-pid foreground
    /// watch is ready; it never injects keys, moves the pointer, or chooses a
    /// user's baseline. The caller owns later window/profile cleanup.
    #[tokio::test]
    #[ignore = "manual owned browser fixture; requires the live-check user baseline"]
    async fn chromium_live_fixture() {
        use super::*;
        use crate::input::inject::foreground_watch::{ForegroundWatch, Target};
        use windows::core::{PCWSTR, PWSTR};
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Threading::{
            CreateProcessW, ResumeThread, CREATE_NO_WINDOW, CREATE_SUSPENDED, PROCESS_INFORMATION,
            STARTF_USESHOWWINDOW, STARTUPINFOW,
        };
        use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, SW_SHOWNOACTIVATE};

        let root = std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap())
            .join("Temp")
            .join("brigadier-cu-test")
            .canonicalize()
            .unwrap();
        let profile = std::path::PathBuf::from(
            std::env::var_os("BRIGADIER_CU_LIVE_PROFILE")
                .expect("explicit new fixture profile is required"),
        );
        assert_eq!(profile.parent().unwrap().canonicalize().unwrap(), root);
        assert!(profile
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("chrome-caret-live-"));
        assert!(
            !profile.exists(),
            "never reuse an existing profile in this fixture"
        );
        std::fs::create_dir(&profile).unwrap();
        let platform = WindowsBrowserPlatform::default();
        let executable = platform.isolated_browser_executable().unwrap();
        assert!(executable.to_ascii_lowercase().ends_with("\\chrome.exe"));
        let mut argv = vec![
            executable.clone(),
            "--new-window".into(),
            format!("--user-data-dir={}", profile.display()),
            "--remote-debugging-port=0".into(),
            "--no-first-run".into(),
            "--no-default-browser-check".into(),
            "--disable-background-mode".into(),
            "http://127.0.0.1:8767/caret".into(),
        ];
        let command = argv
            .drain(..)
            .map(|a| super::super::browser_target::quote_argument(&a))
            .collect::<Vec<_>>()
            .join(" ");
        let mut command: Vec<u16> = command.encode_utf16().chain(Some(0)).collect();
        let startup = STARTUPINFOW {
            cb: std::mem::size_of::<STARTUPINFOW>() as u32,
            dwFlags: STARTF_USESHOWWINDOW,
            wShowWindow: SW_SHOWNOACTIVATE.0 as u16,
            ..Default::default()
        };
        let mut process = PROCESS_INFORMATION::default();
        let prior = unsafe { GetForegroundWindow() }.0 as usize;
        assert_ne!(
            prior, 0,
            "the manual fixture requires an actual selected baseline"
        );
        std::env::set_var("TEMP", &root);
        std::env::set_var("TMP", &root);
        unsafe {
            CreateProcessW(
                PCWSTR::null(),
                PWSTR(command.as_mut_ptr()),
                None,
                None,
                false,
                CREATE_NO_WINDOW | CREATE_SUSPENDED,
                None,
                PCWSTR::null(),
                &startup,
                &mut process,
            )
            .unwrap();
        }
        let watch = ForegroundWatch::start(
            prior,
            Target::Processes(vec![process.dwProcessId], Vec::new()),
            true,
        );
        let resumed = unsafe { ResumeThread(process.hThread) };
        unsafe {
            let _ = CloseHandle(process.hThread);
            let _ = CloseHandle(process.hProcess);
        }
        assert_ne!(resumed, u32::MAX);
        let identity = platform
            .process_fingerprint(process.dwProcessId.into())
            .await
            .unwrap();
        println!(
            "OWN_FIXTURE {}",
            json!({"pid":process.dwProcessId,"profile":profile,"identity":identity,"baseline_hwnd":prior})
        );
        tokio::time::sleep(Duration::from_secs(8)).await;
        println!("OWN_FIXTURE_FOREGROUND {:?}", watch.report());
    }
}
