//! The browser fixture's server (§8, Phase 5): serves the fixture pages on the loopback
//! interface and appends every event a page posts to `/log` to the log file, one JSON line each,
//! the format the native fixture writes. Test-only; the suite and the bench start it and stop it.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};

/// The pages, built into the binary so the server needs no checkout.
const FILES: &[(&str, &str, &str)] = &[
    (
        "/index.html",
        "text/html; charset=utf-8",
        include_str!("../fixtures/web-range/index.html"),
    ),
    (
        "/frame.html",
        "text/html; charset=utf-8",
        include_str!("../fixtures/web-range/frame.html"),
    ),
    (
        "/xframe.html",
        "text/html; charset=utf-8",
        include_str!("../fixtures/web-range/xframe.html"),
    ),
    (
        "/grounding.html",
        "text/html; charset=utf-8",
        include_str!("../fixtures/web-range/grounding.html"),
    ),
    (
        "/log.js",
        "text/javascript; charset=utf-8",
        include_str!("../fixtures/web-range/log.js"),
    ),
];

/// The largest body a page may post.
const MAX_BODY: usize = 64 * 1024;

/// Serves until the process is ended. Writes the port to `port_file` once both loopback
/// addresses listen, so `localhost` and `127.0.0.1` are two sites on one server.
pub fn serve(log: PathBuf, port_file: &Path) -> Result<()> {
    let v4 = TcpListener::bind(("127.0.0.1", 0)).context("binding 127.0.0.1")?;
    let port = v4.local_addr()?.port();
    // `localhost` may resolve to ::1 first; a browser falls back to 127.0.0.1 when it can't.
    let v6 = TcpListener::bind(("::1", port)).ok();
    let log = Arc::new(Mutex::new(log));
    std::fs::write(port_file, format!("{port}\n")).context("writing the port file")?;
    if let Some(v6) = v6 {
        let log = log.clone();
        std::thread::spawn(move || accept(&v6, &log));
    }
    accept(&v4, &log);
    Ok(())
}

fn accept(l: &TcpListener, log: &Arc<Mutex<PathBuf>>) {
    for s in l.incoming().flatten() {
        let log = log.clone();
        std::thread::spawn(move || {
            let _ = handle(s, &log);
        });
    }
}

fn handle(mut s: TcpStream, log: &Mutex<PathBuf>) -> Result<()> {
    let mut r = BufReader::new(s.try_clone()?);
    loop {
        let mut line = String::new();
        if r.read_line(&mut line)? == 0 {
            return Ok(());
        }
        let mut parts = line.split_whitespace();
        let (method, path) = (parts.next().unwrap_or(""), parts.next().unwrap_or("/"));
        let path = path.split('?').next().unwrap_or("/").to_owned();
        let method = method.to_owned();
        let mut length = 0usize;
        loop {
            let mut h = String::new();
            if r.read_line(&mut h)? == 0 || h.trim().is_empty() {
                break;
            }
            if let Some((k, v)) = h.split_once(':')
                && k.trim().eq_ignore_ascii_case("content-length")
            {
                length = v.trim().parse().unwrap_or(0);
            }
        }
        let mut body = vec![0; length.min(MAX_BODY)];
        r.read_exact(&mut body)?;
        let (status, kind, content): (&str, &str, &[u8]) = match (method.as_str(), path.as_str()) {
            ("POST", "/log") => {
                let text = String::from_utf8_lossy(&body);
                // One event per line: a body with a newline in it would forge a second one.
                if serde_json::from_str::<serde_json::Value>(&text).is_ok() && !text.contains('\n')
                {
                    let path = log.lock().map_err(|_| anyhow::anyhow!("log lock"))?;
                    let mut f = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&*path)?;
                    writeln!(f, "{text}")?;
                }
                ("204 No Content", "text/plain", b"")
            }
            ("GET", p) => {
                let p = if p == "/" { "/index.html" } else { p };
                match FILES.iter().find(|(name, _, _)| *name == p) {
                    Some((_, kind, text)) => ("200 OK", kind, text.as_bytes()),
                    None => ("404 Not Found", "text/plain", b"not found"),
                }
            }
            _ => ("405 Method Not Allowed", "text/plain", b""),
        };
        write!(
            s,
            "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\n\r\n",
            content.len()
        )?;
        s.write_all(content)?;
        s.flush()?;
    }
}
