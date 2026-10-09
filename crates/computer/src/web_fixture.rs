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

/// A request line's path.
fn path_of(line: &str) -> &str {
    line.split_whitespace().nth(1).unwrap_or("")
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
        let (mut length, mut close, mut agent) = (0usize, false, String::new());
        loop {
            let mut h = String::new();
            if r.read_line(&mut h)? == 0 || h.trim().is_empty() {
                break;
            }
            if let Some((k, v)) = h.split_once(':')
                && k.trim().eq_ignore_ascii_case("content-length")
            {
                length = v.trim().parse().unwrap_or(0);
            } else if let Some((k, v)) = h.split_once(':')
                && k.trim().eq_ignore_ascii_case("connection")
            {
                close = v.trim().eq_ignore_ascii_case("close");
            } else if let Some((k, v)) = h.split_once(':')
                && k.trim().eq_ignore_ascii_case("user-agent")
            {
                agent = v.trim().to_owned();
            }
        }
        let mut body = vec![0; length.min(MAX_BODY)];
        r.read_exact(&mut body)?;
        // Every browser names itself `Mozilla/…`; anything else (a script posting the log, say)
        // is noted next to the log, so the suite's checker sees it.
        if !agent.starts_with("Mozilla/") {
            let path = log.lock().map_err(|_| anyhow::anyhow!("log lock"))?;
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(format!("{}.foreign", path.display()))?;
            writeln!(f, "{method} {path} {agent:?}", path = path_of(&line))?;
        }
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
        if close {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(port: u16, req: &str) -> String {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.write_all(req.as_bytes()).unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        out
    }

    #[test]
    fn it_serves_the_pages_and_appends_each_logged_event() {
        // Removed when the test ends, passed or not.
        struct Dir(PathBuf);
        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let dir = Dir(std::env::temp_dir().join(format!("cu-web-fixture-{}", std::process::id())));
        std::fs::create_dir_all(&dir.0).unwrap();
        let (log, port_file) = (dir.0.join("log.jsonl"), dir.0.join("port"));
        let (l, p) = (log.clone(), port_file.clone());
        std::thread::spawn(move || serve(l, &p));
        let mut port = None;
        for _ in 0..200 {
            if let Ok(t) = std::fs::read_to_string(&port_file)
                && let Ok(n) = t.trim().parse::<u16>()
            {
                port = Some(n);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let port = port.expect("the server wrote its port");
        let page = request(
            port,
            "GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
        );
        assert!(page.starts_with("HTTP/1.1 200"), "{page}");
        assert!(page.contains("Web Range"));
        let body = r#"{"id":"name","ev":"input","v":"Ada","trusted":true}"#;
        let req = format!(
            "POST /log HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        assert!(request(port, &req).starts_with("HTTP/1.1 2"));
        let logged = std::fs::read_to_string(&log).unwrap();
        assert!(logged.contains(r#""id":"name""#), "{logged}");
        assert!(
            request(port, "GET /nope HTTP/1.1\r\nConnection: close\r\n\r\n")
                .starts_with("HTTP/1.1 404")
        );
    }
}
