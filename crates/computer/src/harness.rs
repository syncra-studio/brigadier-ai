//! The development and fixture harness behind `brigadier-computer` (§4.1). Workers never reach
//! the engine this way; from Phase 2 their calls go through the daemon's broker.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};

use crate::action::{ActRequest, Action, ObserveRequest, Reply, ZoomRequest};
use crate::block::BlockList;
use crate::desktop::Desktop;
use crate::engine::Engine;
use crate::geom::{Point, Provider, Rect};
use crate::redact::Rgba;

/// Resolves `"window": "launched"` to the first window the last launch step opened.
fn launched_window(v: &mut Value, launched: Option<u32>) -> Result<()> {
    if v.get("window").and_then(Value::as_str) == Some("launched") {
        v["window"] = json!(launched.ok_or_else(|| anyhow!("no launch opened a window"))?);
    }
    Ok(())
}

/// Resolves `"window": "<title>"` to a window id.
pub fn resolve_window<D: Desktop>(engine: &mut Engine<D>, v: &mut Value) -> Result<()> {
    if let Some(title) = v.get("window").and_then(Value::as_str).map(str::to_owned) {
        let apps = engine.desktop.apps().map_err(|e| anyhow!("{e}"))?;
        let id = apps
            .iter()
            .flat_map(|a| &a.windows)
            .find(|w| w.title == title)
            .or_else(|| {
                apps.iter()
                    .flat_map(|a| &a.windows)
                    .find(|w| w.title.contains(&title))
            })
            .map(|w| w.id)
            .ok_or_else(|| anyhow!("no window titled {title:?}"))?;
        v["window"] = json!(id);
    }
    Ok(())
}

/// The block list for a development run: nothing hosts it, so only the fixed entries apply.
pub fn dev_block_list() -> BlockList {
    BlockList::default()
}

pub fn decode_png(bytes: &[u8]) -> Result<Rgba> {
    let dec = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut r = dec.read_info()?;
    let mut buf = vec![0; r.output_buffer_size().context("png size")?];
    let info = r.next_frame(&mut buf)?;
    buf.truncate(info.buffer_size());
    if info.color_type != png::ColorType::Rgba {
        bail!("unexpected png colour type");
    }
    Ok(Rgba {
        width: info.width,
        height: info.height,
        data: buf,
    })
}

fn print_reply(reply: &Reply, out: Option<&Path>) -> Result<()> {
    print!("{}", reply.text);
    if let (Some(img), Some(dir)) = (&reply.image, out) {
        let path = dir.join(format!("{}.png", img.id));
        std::fs::write(&path, &img.png)?;
        println!("  [{} saved to {}]", img.id, path.display());
    }
    Ok(())
}

/// Runs a script: `{"worker": "...", "foreground": false, "steps": [{"launch": {...}} | {"observe": {...}} | {"act": {...}} | {"zoom": {...}}]}`.
/// `foreground: false` keeps the foreground rung off, so the script never raises a window or
/// takes the front, however long the user has been idle.
/// Writes images, the action records and an annotated copy of the last full screenshot with every
/// predicted point to `out`.
pub fn run_script<D: Desktop>(engine: &mut Engine<D>, script: &Value, out: &Path) -> Result<()> {
    std::fs::create_dir_all(out)?;
    let worker = script
        .get("worker")
        .and_then(Value::as_str)
        .unwrap_or("dev")
        .to_owned();
    let steps = script
        .get("steps")
        .and_then(Value::as_array)
        .context("steps")?;
    if let Some(on) = script.get("foreground").and_then(Value::as_bool) {
        engine.foreground = on;
    }
    let mut last_full: Option<(String, Rgba)> = None;
    let mut annotated: Option<Rgba> = None;
    let mut launched: Option<u32> = None;
    for (i, step) in steps.iter().enumerate() {
        println!("── step {} ──", i + 1);
        if let Some(o) = step.get("observe") {
            let mut o = o.clone();
            launched_window(&mut o, launched)?;
            resolve_window(engine, &mut o)?;
            let req: ObserveRequest = serde_json::from_value(o)?;
            let reply = engine.observe(&worker, &req).map_err(|e| anyhow!("{e}"))?;
            print_reply(&reply, Some(out))?;
            if let Some(img) = &reply.image {
                let rgba = decode_png(&img.png)?;
                annotated = Some(rgba.clone());
                last_full = Some((img.id.clone(), rgba));
            }
        } else if let Some(a) = step.get("act") {
            let mut a = a.clone();
            launched_window(&mut a, launched)?;
            resolve_window(engine, &mut a)?;
            let req: ActRequest = serde_json::from_value(a)?;
            // Mark where each action is predicted to land, before it runs.
            if let (Some((id, _)), Some(canvas)) = (&last_full, annotated.as_mut()) {
                for act in &req.actions {
                    if let Some((p, b)) = predicted(engine, req.window, id, act) {
                        canvas.mark(p, b);
                    }
                }
            }
            let reply = engine.act(&worker, &req).map_err(|e| anyhow!("{e}"))?;
            print_reply(&reply, Some(out))?;
        } else if let Some(l) = step.get("launch") {
            let req: crate::wire::LaunchRequest = serde_json::from_value(l.clone())?;
            let cancel = engine
                .gens
                .token(&worker, std::time::Duration::from_secs(30));
            let o = crate::launch::launch(engine, &req, &cancel).map_err(|e| anyhow!("{e}"))?;
            println!(
                "launched {} pid {} (new process: {}) windows {:?}",
                o.app.name, o.app.pid, o.new_process, o.new_windows
            );
            launched = o.new_windows.first().copied();
        } else if let Some(z) = step.get("zoom") {
            let req: ZoomRequest = serde_json::from_value(z.clone())?;
            let reply = engine.zoom(&worker, &req).map_err(|e| anyhow!("{e}"))?;
            print_reply(&reply, Some(out))?;
        } else {
            bail!("unknown step {step}");
        }
    }
    let mut records = String::new();
    for r in engine.records.drain(..) {
        records.push_str(&serde_json::to_string(&r)?);
        records.push('\n');
    }
    std::fs::write(out.join("actions.jsonl"), records)?;
    if let Some(img) = annotated {
        std::fs::write(out.join("annotated.png"), img.encode_png())?;
    }
    println!("records and annotated screenshot in {}", out.display());
    Ok(())
}

/// Where an action will land on the image `image`: the point, and the element's box for refs.
fn predicted<D: Desktop>(
    engine: &Engine<D>,
    window: u32,
    image: &str,
    act: &Action,
) -> Option<(Point, Option<Rect>)> {
    let t = engine.image_transform(image)?;
    let target = match act {
        Action::Click { target, .. } | Action::Scroll { target, .. } => target.clone(),
        Action::Drag { from, .. } => from.clone(),
        Action::SetValue { r#ref, .. } | Action::Perform { r#ref, .. } => crate::action::Target {
            r#ref: Some(r#ref.clone()),
            ..Default::default()
        },
        Action::Type { r#ref: Some(r), .. } => crate::action::Target {
            r#ref: Some(r.clone()),
            ..Default::default()
        },
        _ => return None,
    };
    if let Some(r) = target.r#ref.as_deref().and_then(crate::tree::parse_ref) {
        let f = engine.ref_frame(window, r)?;
        let a = t.to_image(Point::new(f.x, f.y));
        let c = t.to_image(f.center());
        return Some((c, Some(Rect::new(a.x, a.y, f.w * t.scale, f.h * t.scale))));
    }
    match (target.image.as_deref(), target.x, target.y) {
        (Some(id), Some(x), Some(y)) if id == image => Some((Point::new(x, y), None)),
        (Some(id), Some(x), Some(y)) => {
            let other = engine.image_transform(id)?;
            let p = Point::new(
                other.crop.x + x / other.scale,
                other.crop.y + y / other.scale,
            );
            Some((t.to_image(p), None))
        }
        _ => None,
    }
}

/// The default output folder for harness artefacts.
pub fn out_dir(root: Option<PathBuf>, name: &str) -> PathBuf {
    root.unwrap_or_else(|| PathBuf::from("target/computer-bench"))
        .join(name)
}

pub fn new_engine<D: Desktop>(desktop: D) -> Engine<D> {
    Engine::new(desktop, dev_block_list(), Provider::Claude)
}
