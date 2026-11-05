use std::io::{self, Write};
use std::path::{Path, PathBuf};

use brigadier_core::traces;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{InitError, RollingFileAppender, Rotation};
use tracing_subscriber::fmt::writer::MakeWriterExt;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{EnvFilter, Layer};

/// Environment variable with a `tracing` filter, e.g. `brigadier=debug`.
const FILTER_ENV: &str = "BRIGADIER_LOG";
const KEPT_LOG_FILES: usize = 7;

/// Today's log file, reopened when a Delete's scrub has replaced a log file. The appender alone
/// keeps its open file until the day ends, so it would go on writing to the replaced one.
struct LogFile {
    dir: PathBuf,
    appender: RollingFileAppender,
    swaps: u64,
}

impl LogFile {
    fn open(dir: &Path) -> Result<Self, InitError> {
        // Read before opening: a swap in between only reopens once more.
        let swaps = traces::swaps();
        Ok(Self {
            dir: dir.to_owned(),
            appender: appender(dir)?,
            swaps,
        })
    }
}

impl Write for LogFile {
    fn write(&mut self, line: &[u8]) -> io::Result<usize> {
        let swaps = traces::swaps();
        if swaps != self.swaps {
            match appender(&self.dir) {
                Ok(appender) => {
                    self.appender = appender;
                    self.swaps = swaps;
                }
                Err(err) => eprintln!("could not reopen the log file: {err}"),
            }
        }
        self.appender.write(line)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.appender.flush()
    }
}

fn appender(dir: &Path) -> Result<RollingFileAppender, InitError> {
    RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("brigadierd")
        .filename_suffix("log")
        .max_log_files(KEPT_LOG_FILES)
        .build(dir)
}

/// Structured JSON logs in daily files under `logs_dir`, written by a background thread so
/// logging never blocks the runtime. In the foreground, logs also go to stderr.
pub fn init(logs_dir: &Path, foreground: bool) -> anyhow::Result<WorkerGuard> {
    std::fs::create_dir_all(logs_dir)?;
    let (file, guard) = tracing_appender::non_blocking(LogFile::open(logs_dir)?);
    let filter = EnvFilter::try_from_env(FILTER_ENV).unwrap_or_else(|_| EnvFilter::new("info"));
    let layer = tracing_subscriber::fmt::layer()
        .json()
        .with_current_span(false)
        .with_thread_names(true);
    if foreground {
        tracing_subscriber::registry()
            .with(
                layer
                    .with_writer(file.and(std::io::stderr))
                    .with_filter(filter),
            )
            .init();
    } else {
        tracing_subscriber::registry()
            .with(layer.with_writer(file).with_filter(filter))
            .init();
    }

    // Panics anywhere (any thread, any task) are recorded before the default hook runs.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        tracing::error!(
            thread = thread.name().unwrap_or("unnamed"),
            panic = %info,
            "panic"
        );
        default_hook(info);
    }));
    Ok(guard)
}

macro_rules! private_trace {
    ($private:expr, $level:ident, $message:literal; $($fields:tt)*) => {
        if $private {
            tracing::$level!(conversation = "tab chat", $message);
        } else {
            tracing::$level!($($fields)*, $message);
        }
    };
}

pub(crate) use private_trace;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn after_a_scrub_the_daemon_goes_on_writing_to_the_file_people_read() {
        let dir = std::env::temp_dir().join(format!(
            "brigadier-log-reopen-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mut log = LogFile::open(&dir).unwrap();
        let id = "aaaaaaaa-session";
        log.write_all(format!("kept\n{id}\n").as_bytes()).unwrap();
        log.flush().unwrap();
        assert_eq!(traces::scrub_logs(&dir, &[id.to_owned()]).unwrap(), 1);
        log.write_all(b"after\n").unwrap();
        log.flush().unwrap();
        let files: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().collect();
        assert_eq!(files.len(), 1);
        assert_eq!(
            std::fs::read_to_string(files[0].path()).unwrap(),
            "kept\nafter\n"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
