//! The agent cursor's live proofs (docs/COMPUTER-USE-PLAN.md §4.5, Phase 3), on two copies of
//! the bench's fixture app it launches itself and quits at the end. It sends input to nothing
//! else.
//!
//!   cargo run --release -p brigadier-computer --example cursor-proof -- <out dir>
//!
//! 1. Two workers at once: a helper (`brigadier-computer serve`, built beside this example) and
//!    two connections, one per worker, each with its own name, act together on their own copy:
//!    one presses buttons through accessibility, the other clicks the dots by pixel. Midway the
//!    screen is saved as `two-workers.png`, with both cursors on it.
//! 2. Never in a capture: a worker's window observed with its cursor parked over it, then again
//!    after the worker ended and its cursor went: the two screenshots are the same bytes. Then
//!    the same for the action log's image, in this process: one click while the cursor sits on
//!    the point it aims at, one after it faded.
//!
//! Needs Accessibility and Screen Recording for the process that runs it (a terminal's grants
//! carry over to what it launches).

#[cfg(target_os = "macos")]
fn main() -> anyhow::Result<()> {
    proof::main()
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("cursor-proof: macOS only");
}

#[cfg(target_os = "macos")]
mod proof {
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::sync::Arc;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use anyhow::{Context, Result, anyhow, bail, ensure};
    use brigadier_computer::action::{
        ActRequest, Action, ObserveRequest, Screenshot, Status, Target,
    };
    use brigadier_computer::client::{Answer, Client};
    use brigadier_computer::cursor::CursorSink;
    use brigadier_computer::desktop::{Desktop, WindowInfo};
    use brigadier_computer::geom::Provider;
    use brigadier_computer::wire::{Op, Policy};
    use brigadier_computer::{harness, macos, tree};

    /// The fixture's dot canvas in its window's content, and its content height (`main.swift`).
    const CANVAS: (f64, f64) = (580.0, 10.0);
    const CONTENT_HEIGHT: f64 = 600.0;
    const DOTS: [(f64, f64); 4] = [(60.0, 120.0), (200.0, 160.0), (60.0, 300.0), (200.0, 340.0)];
    const BUTTONS: [&str; 4] = [
        "Button 8 pt",
        "Button 12 pt",
        "Button 16 pt",
        "Button 24 pt",
    ];

    /// A process this proof started, killed by its own pid when dropped.
    struct Owned(Child);

    impl Drop for Owned {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    pub fn main() -> Result<()> {
        let out = PathBuf::from(std::env::args().nth(1).context("cursor-proof <out dir>")?);
        std::fs::create_dir_all(&out)?;
        let (ax, screen) = macos::permissions();
        ensure!(ax && screen, "needs Accessibility and Screen Recording");
        let mtm = objc2::MainThreadMarker::new().context("the main thread")?;
        macos::overlay::run_with_overlay(mtm, move |overlay| match run(&out, overlay) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("cursor-proof: {e:#}");
                1
            }
        })
    }

    fn run(out: &Path, overlay: Arc<macos::overlay::Overlay>) -> Result<()> {
        let fixture = build_fixture(out)?;
        let a = launch(&fixture, &out.join("a.jsonl"))?;
        let b = launch(&fixture, &out.join("b.jsonl"))?;
        let mut desktop = brigadier_computer::system_desktop().map_err(|e| anyhow!("{e}"))?;
        let win_a = main_window(&mut desktop, a.0.id() as i32)?;
        let win_b = main_window(&mut desktop, b.0.id() as i32)?;

        // The helper, as the daemon starts it, beside this example in target/<profile>.
        let exe = std::env::current_exe()?;
        let helper_bin = exe
            .parent()
            .and_then(Path::parent)
            .map(|d| d.join("brigadier-computer"))
            .context("the helper's path")?;
        ensure!(
            helper_bin.exists(),
            "build the helper first: cargo build --release -p brigadier-computer"
        );
        // The helper keeps its socket's folder to itself (0700).
        let socket = out.join("helper").join("socket");
        let token_file = out.join("token");
        let _ = std::fs::remove_file(&token_file);
        let helper = Owned(
            Command::new(&helper_bin)
                .args(["serve", "--socket"])
                .arg(&socket)
                .arg("--token-file")
                .arg(&token_file)
                .stdin(Stdio::null())
                .spawn()
                .context("starting the helper")?,
        );
        let token = wait_for(Duration::from_secs(5), || {
            std::fs::read_to_string(&token_file)
                .ok()
                .filter(|t| !t.trim().is_empty())
        })
        .context("the helper's token")?;
        let client = Arc::new(Client::connect(&socket, token.trim(), |_| {})?);

        // 1. Two workers at once.
        let started = Instant::now();
        let shot = out.join("two-workers.png");
        let worker = |name: &'static str, label: &'static str, win: WindowInfo, pixels: bool| {
            let client = client.clone();
            std::thread::spawn(move || -> Result<u32> {
                let call = Caller {
                    client: &client,
                    worker: name,
                    label,
                    pid: win.pid,
                };
                let mut done = 0;
                let title_bar = win.frame.h - CONTENT_HEIGHT;
                for round in 0..12 {
                    let seen = call.observe(win.id, pixels)?;
                    let action = if pixels {
                        let image = seen.reply.image.as_ref().context("no image")?;
                        let scale = f64::from(image.width) / win.frame.w;
                        let (x, y) = DOTS[round % DOTS.len()];
                        Action::Click {
                            target: Target {
                                image: Some(image.id.clone()),
                                x: Some((CANVAS.0 + x) * scale),
                                y: Some((title_bar + CANVAS.1 + y) * scale),
                                ..Default::default()
                            },
                            button: Default::default(),
                            count: 1,
                            modifiers: Vec::new(),
                            expect: None,
                        }
                    } else {
                        Action::Click {
                            target: Target {
                                r#ref: Some(find_ref(&seen.reply.text, BUTTONS[round % 4])?),
                                ..Default::default()
                            },
                            button: Default::default(),
                            count: 1,
                            modifiers: Vec::new(),
                            expect: None,
                        }
                    };
                    let acted = call.act(win.id, action)?;
                    if acted.reply.ok
                        && acted.reply.results.iter().all(|r| r.status == Status::Done)
                    {
                        done += 1;
                    }
                    std::thread::sleep(Duration::from_millis(350));
                }
                Ok(done)
            })
        };
        let ta = worker("task-a", "Fix the login page", win_a.clone(), false);
        let tb = worker("task-b", "Check the release notes", win_b.clone(), true);
        std::thread::sleep(Duration::from_millis(2500));
        let saved = Command::new("screencapture")
            .arg("-x")
            .arg(&shot)
            .status()?;
        ensure!(saved.success(), "screencapture failed");
        let done_a = ta.join().map_err(|_| anyhow!("worker a panicked"))??;
        let done_b = tb.join().map_err(|_| anyhow!("worker b panicked"))??;
        println!(
            "two workers: {done_a}/12 and {done_b}/12 actions done in {:.1} s; screen saved at {}",
            started.elapsed().as_secs_f64(),
            shot.display()
        );

        // 2a. The worker's window with its cursor parked over it, then after its session ended.
        let b_call = Caller {
            client: &client,
            worker: "task-b",
            label: "Check the release notes",
            pid: win_b.pid,
        };
        let with_cursor = b_call.observe(win_b.id, true)?;
        let parked = out.join("cursor-parked.png");
        Command::new("screencapture")
            .arg("-x")
            .arg(&parked)
            .status()?;
        client.tell("task-b", Op::EndSession);
        std::thread::sleep(Duration::from_millis(800));
        let without = b_call.observe(win_b.id, true)?;
        let (one, two) = (
            with_cursor.image.context("no image")?,
            without.image.context("no image")?,
        );
        std::fs::write(out.join("observe-with-cursor.png"), &one)?;
        std::fs::write(out.join("observe-without-cursor.png"), &two)?;
        println!(
            "observe with the cursor parked over the window and without it: {} and {} bytes, {}",
            one.len(),
            two.len(),
            if one == two { "identical" } else { "DIFFERENT" }
        );
        client.tell("task-a", Op::EndSession);
        drop(helper);

        // 2b. The action log's image, in this process: the same click with the cursor on its
        // point, then after the cursor faded (the overlay fades a cursor 5 s after its last aim).
        let mut engine = harness::new_engine(desktop);
        engine.desktop.watch(win_b.pid);
        overlay.label("proof", "Capture check");
        let title_bar = win_b.frame.h - CONTENT_HEIGHT;
        let mut trajectory = |cursor: bool| -> Result<Vec<u8>> {
            engine.cursor = cursor.then(|| overlay.clone() as Arc<dyn CursorSink>);
            let seen = engine
                .observe(
                    "proof",
                    &ObserveRequest {
                        window: win_b.id,
                        screenshot: Screenshot::Always,
                        since: None,
                        full: false,
                        element: None,
                        find: None,
                        value_page: None,
                    },
                )
                .map_err(|e| anyhow!("{e}"))?;
            let image = seen.image.context("no image")?;
            let scale = f64::from(image.width) / win_b.frame.w;
            let reply = engine
                .act(
                    "proof",
                    &ActRequest {
                        window: win_b.id,
                        actions: vec![Action::Click {
                            target: Target {
                                image: Some(image.id),
                                x: Some((CANVAS.0 + DOTS[0].0) * scale),
                                y: Some((title_bar + CANVAS.1 + DOTS[0].1) * scale),
                                ..Default::default()
                            },
                            button: Default::default(),
                            count: 1,
                            modifiers: Vec::new(),
                            expect: None,
                        }],
                        screenshot: Screenshot::Never,
                    },
                )
                .map_err(|e| anyhow!("{e}"))?;
            Ok(reply.trajectory.context("no trajectory")?.png)
        };
        // The first click glides the cursor there; the second one acts with it parked on the point.
        trajectory(true)?;
        std::thread::sleep(Duration::from_millis(700));
        let on = trajectory(true)?;
        let parked_log = out.join("trajectory-parked.png");
        Command::new("screencapture")
            .arg("-x")
            .arg(&parked_log)
            .status()?;
        overlay.end("proof");
        std::thread::sleep(Duration::from_millis(800));
        let off = trajectory(false)?;
        std::fs::write(out.join("trajectory-with-cursor.png"), &on)?;
        std::fs::write(out.join("trajectory-without-cursor.png"), &off)?;
        println!(
            "action-log image with the cursor on the point and without it: {} and {} bytes, {}",
            on.len(),
            off.len(),
            if on == off { "identical" } else { "DIFFERENT" }
        );
        drop(a);
        drop(b);
        if one != two || on != off {
            bail!("a capture differs with the cursor shown");
        }
        Ok(())
    }

    /// One worker's requests over the shared connection, waited for.
    struct Caller<'a> {
        client: &'a Client,
        worker: &'static str,
        label: &'static str,
        pid: i32,
    }

    impl Caller<'_> {
        fn call(&self, op: Op) -> Result<Answer> {
            let (tx, rx) = mpsc::channel();
            let policy = Policy {
                launched_pids: vec![self.pid],
                label: Some(self.label.into()),
                ..Default::default()
            };
            self.client.send(
                self.client.next_id(),
                self.worker,
                Provider::Claude,
                policy,
                op,
                move |a| {
                    let _ = tx.send(a);
                },
            );
            rx.recv_timeout(Duration::from_secs(20))?
                .map_err(|_| anyhow!("the helper went away"))
        }

        fn observe(&self, window: u32, shot: bool) -> Result<Answer> {
            let a = self.call(Op::Observe(ObserveRequest {
                window,
                screenshot: if shot {
                    Screenshot::Always
                } else {
                    Screenshot::Never
                },
                since: None,
                full: true,
                element: None,
                find: None,
                value_page: None,
            }))?;
            ensure!(a.reply.ok, "observe: {:?}", a.reply.error);
            Ok(a)
        }

        fn act(&self, window: u32, action: Action) -> Result<Answer> {
            self.call(Op::Act(ActRequest {
                window,
                actions: vec![action],
                screenshot: Screenshot::Never,
            }))
        }
    }

    fn find_ref(text: &str, needle: &str) -> Result<String> {
        text.lines()
            .find(|l| l.contains(needle))
            .and_then(|l| l.split_whitespace().find(|w| tree::parse_ref(w).is_some()))
            .map(str::to_owned)
            .ok_or_else(|| anyhow!("no element {needle:?} in the observation"))
    }

    fn wait_for<T>(wait: Duration, mut found: impl FnMut() -> Option<T>) -> Option<T> {
        let end = Instant::now() + wait;
        loop {
            if let Some(t) = found() {
                return Some(t);
            }
            if Instant::now() > end {
                return None;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn build_fixture(dir: &Path) -> Result<PathBuf> {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/target-range/main.swift");
        let bin = dir.join("target-range");
        let st = Command::new("swiftc")
            .arg("-O")
            .arg(&src)
            .arg("-o")
            .arg(&bin)
            .status()?;
        ensure!(st.success(), "the fixture didn't build");
        Ok(bin)
    }

    fn launch(bin: &Path, log: &Path) -> Result<Owned> {
        Ok(Owned(
            Command::new(bin)
                .arg(log)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()?,
        ))
    }

    fn main_window(desktop: &mut macos::MacDesktop, pid: i32) -> Result<WindowInfo> {
        let w = wait_for(Duration::from_secs(10), || {
            desktop
                .windows(pid)
                .ok()?
                .into_iter()
                .find(|w| w.title == "Target Range")
        })
        .context("the fixture's window didn't appear")?;
        // Its own window notices settle first.
        std::thread::sleep(Duration::from_millis(500));
        Ok(w)
    }
}
