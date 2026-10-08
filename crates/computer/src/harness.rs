//! The development and fixture harness behind `brigadier-computer` (§4.1). Workers never reach
//! the engine this way; from Phase 2 their calls go through the daemon's broker.

use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};

use crate::action::{ActRequest, Action, ObserveRequest, Reply, ZoomRequest};
use crate::block::BlockList;
use crate::desktop::Desktop;
use crate::engine::Engine;
use crate::geom::{Point, Provider, Rect};
use crate::redact::Rgba;

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

/// Draws a predicted point: a red ring and cross.
pub fn mark(img: &mut Rgba, p: Point, label_box: Option<Rect>) {
    let red = [230, 20, 40, 255];
    for t in 0..360 {
        let a = f64::from(t).to_radians();
        for r in [7.0, 8.0] {
            img.fill(
                Rect::new(p.x + r * a.cos(), p.y + r * a.sin(), 1.0, 1.0),
                red,
            );
        }
    }
    img.fill(Rect::new(p.x - 12.0, p.y, 24.0, 1.0), red);
    img.fill(Rect::new(p.x, p.y - 12.0, 1.0, 24.0), red);
    if let Some(b) = label_box {
        img.fill(Rect::new(b.x, b.y, b.w, 1.0), red);
        img.fill(Rect::new(b.x, b.y + b.h, b.w, 1.0), red);
        img.fill(Rect::new(b.x, b.y, 1.0, b.h), red);
        img.fill(Rect::new(b.x + b.w, b.y, 1.0, b.h), red);
    }
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

/// Runs a script: `{"worker": "...", "steps": [{"observe": {...}} | {"act": {...}} | {"zoom": {...}}]}`.
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
    let mut last_full: Option<(String, Rgba)> = None;
    let mut annotated: Option<Rgba> = None;
    for (i, step) in steps.iter().enumerate() {
        println!("── step {} ──", i + 1);
        if let Some(o) = step.get("observe") {
            let mut o = o.clone();
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
            resolve_window(engine, &mut a)?;
            let req: ActRequest = serde_json::from_value(a)?;
            // Mark where each action is predicted to land, before it runs.
            if let (Some((id, _)), Some(canvas)) = (&last_full, annotated.as_mut()) {
                for act in &req.actions {
                    if let Some((p, b)) = predicted(engine, req.window, id, act) {
                        mark(canvas, p, b);
                    }
                }
            }
            let reply = engine.act(&worker, &req).map_err(|e| anyhow!("{e}"))?;
            print_reply(&reply, Some(out))?;
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

// ── serve ──────────────────────────────────────────────────────────────────────────────────

fn read_frame(s: &mut UnixStream) -> std::io::Result<Vec<u8>> {
    let mut len = [0u8; 4];
    s.read_exact(&mut len)?;
    let n = u32::from_be_bytes(len) as usize;
    if n > 16 << 20 {
        return Err(std::io::Error::other("frame too large"));
    }
    let mut buf = vec![0; n];
    s.read_exact(&mut buf)?;
    Ok(buf)
}

fn write_frame(s: &mut UnixStream, b: &[u8]) -> std::io::Result<()> {
    s.write_all(&(b.len() as u32).to_be_bytes())?;
    s.write_all(b)
}

struct Job {
    worker: String,
    op: String,
    req: Value,
    reply: mpsc::Sender<(Value, Option<Vec<u8>>)>,
}

fn random_token() -> Result<String> {
    let mut b = [0u8; 24];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut b)?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

/// `serve`: frames are a 4-byte big-endian length and JSON; an image follows its reply as one
/// binary frame. Every request carries the token from the token file. `stop` is answered on
/// the connection's own thread, so it never waits behind a running batch (§4.7).
pub fn serve<D: Desktop>(mut engine: Engine<D>, socket: &Path, token_file: &Path) -> Result<()> {
    let dir = socket.parent().context("socket path")?;
    std::fs::create_dir_all(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    let _ = std::fs::remove_file(socket);
    let token = random_token()?;
    std::fs::write(token_file, &token)?;
    std::fs::set_permissions(token_file, std::fs::Permissions::from_mode(0o600))?;
    let listener = UnixListener::bind(socket)?;
    let (tx, rx) = mpsc::channel::<Job>();
    let gens = engine.gens.clone();
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut conn) = conn else { continue };
            let tx = tx.clone();
            let token = token.clone();
            let gens = gens.clone();
            std::thread::spawn(move || {
                while let Ok(frame) = read_frame(&mut conn) {
                    let Ok(msg) = serde_json::from_slice::<Value>(&frame) else {
                        break;
                    };
                    if msg.get("token").and_then(Value::as_str) != Some(token.as_str()) {
                        let _ = write_frame(&mut conn, br#"{"ok":false,"error":"bad token"}"#);
                        break;
                    }
                    let worker = msg
                        .get("worker")
                        .and_then(Value::as_str)
                        .unwrap_or("dev")
                        .to_owned();
                    let op = msg
                        .get("op")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned();
                    if op == "stop" {
                        gens.stop_all();
                        let _ = write_frame(&mut conn, br#"{"ok":true,"text":"stopped\n"}"#);
                        continue;
                    }
                    let (rtx, rrx) = mpsc::channel();
                    let req = msg.get("req").cloned().unwrap_or(Value::Null);
                    if tx
                        .send(Job {
                            worker,
                            op,
                            req,
                            reply: rtx,
                        })
                        .is_err()
                    {
                        break;
                    }
                    let Ok((v, img)) = rrx.recv() else { break };
                    let Ok(bytes) = serde_json::to_vec(&v) else {
                        break;
                    };
                    if write_frame(&mut conn, &bytes).is_err() {
                        break;
                    }
                    if let Some(img) = img
                        && write_frame(&mut conn, &img).is_err()
                    {
                        break;
                    }
                }
            });
        }
    });
    eprintln!("serving on {}", socket.display());
    // The engine stays on this thread: its accessibility objects and run loop live here.
    while let Ok(job) = rx.recv() {
        let answer = handle(&mut engine, &job.worker, &job.op, job.req);
        let _ = job.reply.send(answer);
    }
    Ok(())
}

fn handle<D: Desktop>(
    engine: &mut Engine<D>,
    worker: &str,
    op: &str,
    mut req: Value,
) -> (Value, Option<Vec<u8>>) {
    if let Err(e) = resolve_window(engine, &mut req) {
        return (json!({"ok": false, "error": e.to_string()}), None);
    }
    let reply = match op {
        "apps" => engine.apps_text().map(|text| Reply {
            text,
            image: None,
            results: Vec::new(),
        }),
        "observe" => match serde_json::from_value::<ObserveRequest>(req) {
            Ok(r) => engine.observe(worker, &r),
            Err(e) => return (json!({"ok": false, "error": e.to_string()}), None),
        },
        "act" => match serde_json::from_value::<ActRequest>(req) {
            Ok(r) => engine.act(worker, &r),
            Err(e) => return (json!({"ok": false, "error": e.to_string()}), None),
        },
        "zoom" => match serde_json::from_value::<ZoomRequest>(req) {
            Ok(r) => engine.zoom(worker, &r),
            Err(e) => return (json!({"ok": false, "error": e.to_string()}), None),
        },
        other => {
            return (
                json!({"ok": false, "error": format!("unknown op {other}")}),
                None,
            );
        }
    };
    match reply {
        Ok(r) => {
            let image = r.image.as_ref().map(|i| json!({"id": i.id, "width": i.width, "height": i.height, "tokens": i.tokens, "bytes": i.png.len()}));
            (
                json!({"ok": true, "text": r.text, "results": r.results, "image": image}),
                r.image.map(|i| i.png),
            )
        }
        Err(e) => (
            json!({"ok": false, "error": e.to_string(), "code": e.code}),
            None,
        ),
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
