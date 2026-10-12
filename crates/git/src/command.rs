use std::{
    ffi::{OsStr, OsString},
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{Environment, Error, Oid, Repo, Result, Worktree, parse};

/// The repository a folder belongs to, as [`Git::find_repo`] finds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundRepo {
    /// The main checkout's canonical top level.
    pub root: PathBuf,
    /// The `origin` remote's URL.
    pub origin: Option<String>,
}

/// The user's git binary and captured login-shell environment.
#[derive(Debug, Clone)]
pub struct Git {
    program: PathBuf,
    env: Environment,
    /// Its version, once found new enough: every [`Git::open`] checks it, without asking git
    /// again each time.
    checked_version: Arc<OnceLock<String>>,
}

impl Git {
    /// Use the resolved git binary and login environment supplied by the application.
    pub fn new(program: PathBuf, env: Vec<(OsString, OsString)>) -> Self {
        Self {
            program,
            env,
            checked_version: Arc::default(),
        }
    }

    /// Report git's version and reject versions older than 2.38.
    pub fn version(&self) -> Result<String> {
        if let Some(version) = self.checked_version.get() {
            return Ok(version.clone());
        }
        let version = self.ask_version()?;
        Ok(self.checked_version.get_or_init(|| version).clone())
    }

    fn ask_version(&self) -> Result<String> {
        let output = self.checked(None, &["--version"], true, &[], None)?;
        let version = parse::line(&output)?;
        let number = version
            .strip_prefix("git version ")
            .ok_or_else(|| Error::Parse(version.clone()))?;
        let mut parts = number.split('.');
        let major = parts.next().and_then(|s| s.parse::<u32>().ok());
        let minor = parts.next().and_then(|s| s.parse::<u32>().ok());
        match (major, minor) {
            (Some(major), Some(minor)) if (major, minor) >= (2, 38) => Ok(version),
            (Some(_), Some(_)) => Err(Error::TooOld {
                found: number.to_owned(),
            }),
            _ => Err(Error::Parse(version)),
        }
    }

    /// Open precisely the supplied checkout top level, including an unborn repository.
    pub fn open(&self, path: &Path) -> Result<Repo> {
        self.version()?;
        let canonical = fs::canonicalize(path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::NotARepository(path.to_owned())
            } else {
                e.into()
            }
        })?;
        let out = self.run(
            Some(&canonical),
            &["rev-parse", "--show-toplevel"],
            true,
            &[],
            None,
        )?;
        if !out.status.success() {
            return Err(Error::NotARepository(path.to_owned()));
        }
        let root = fs::canonicalize(parse::path_line(&out.stdout)?)?;
        if root != canonical {
            return Err(Error::NotARepository(path.to_owned()));
        }
        let common = self.checked(
            Some(&root),
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
            true,
            &[],
            None,
        )?;
        let common_dir = fs::canonicalize(parse::path_line(&common)?)?;
        Ok(Repo {
            git: self.clone(),
            root,
            common_dir,
        })
    }

    /// The repository a folder is in: its main checkout's top level (also for a folder in a
    /// linked worktree) and its `origin` URL. `None` when the folder is in no repository.
    pub fn find_repo(&self, path: &Path) -> Result<Option<FoundRepo>> {
        let out = self.run(
            Some(path),
            &[
                "rev-parse",
                "--path-format=absolute",
                "--show-toplevel",
                "--git-common-dir",
            ],
            true,
            &[],
            None,
        )?;
        if !out.status.success() {
            return Ok(None);
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let mut lines = text.lines();
        let (Some(toplevel), Some(common)) = (lines.next(), lines.next()) else {
            return Ok(None);
        };
        let common = Path::new(common);
        // A non-bare repository keeps its metadata in `<main checkout>/.git`.
        let root = match common.file_name() {
            Some(name) if name == ".git" => common.parent().unwrap_or(common),
            _ => Path::new(toplevel),
        };
        let root = fs::canonicalize(root)?;
        let origin = self.run(
            Some(&root),
            &["config", "--get", "remote.origin.url"],
            true,
            &[],
            None,
        )?;
        let origin = String::from_utf8_lossy(&origin.stdout).trim().to_owned();
        Ok(Some(FoundRepo {
            root,
            origin: (!origin.is_empty()).then_some(origin),
        }))
    }

    /// Makes `dir` (created when missing) a new repository with an empty first commit, so
    /// sessions can branch from it. When git can't make the commit (no user identity, a
    /// signing key that needs a passphrase) the branch stays unborn and its error is returned.
    pub fn init(&self, dir: &Path) -> Result<Option<Error>> {
        fs::create_dir_all(dir)?;
        self.checked(Some(dir), &["init", "--quiet"], false, &[], None)?;
        let commit = ["commit", "--quiet", "--allow-empty", "-m", "Initial commit"];
        let out = self.run(Some(dir), &commit, false, &[], None)?;
        Ok((!out.status.success()).then(|| failure(&commit, &out)))
    }

    /// Clones `url` into the new folder `dest`. Never prompts: a remote that needs a password
    /// the credential helper doesn't have fails instead.
    pub fn clone_into(&self, url: &str, dest: &Path) -> Result<()> {
        let args = [
            OsStr::new("clone"),
            OsStr::new("--quiet"),
            OsStr::new("--"),
            OsStr::new(url),
            dest.as_os_str(),
        ];
        self.checked(None, &args, false, &[], None)?;
        Ok(())
    }

    /// Open a task/session checkout that the application created.
    pub fn open_worktree(&self, path: &Path) -> Result<Worktree> {
        let repo = self.open(path)?;
        Ok(Worktree::new(repo))
    }

    pub(crate) fn run<S: AsRef<OsStr>>(
        &self,
        path: Option<&Path>,
        args: &[S],
        read_only: bool,
        extra: &[(OsString, OsString)],
        input: Option<&[u8]>,
    ) -> Result<Output> {
        let mut command = Command::new(&self.program);
        command.env_clear().envs(self.env.iter().cloned());
        // A shell launched inside another git operation must not redirect this repository.
        for key in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_COMMON_DIR",
            "GIT_INDEX_FILE",
            "GIT_PREFIX",
            "GIT_OBJECT_DIRECTORY",
            "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        ] {
            command.env_remove(key);
        }
        command
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("LC_ALL", "C")
            .env("GIT_LITERAL_PATHSPECS", "1")
            .envs(extra.iter().cloned())
            .arg("--no-pager")
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if extra.iter().any(|(key, _)| key == "GIT_INDEX_FILE") {
            command.args(["-c", "core.splitIndex=false"]);
        }
        command.args(args);
        if read_only {
            command.env("GIT_OPTIONAL_LOCKS", "0");
        }
        if let Some(path) = path {
            command.current_dir(path);
        }
        let mut child = command.spawn()?;
        // Write concurrently so filters/hooks emitting output cannot deadlock a large input.
        if let Some(input) = input {
            let mut stdin = child
                .stdin
                .take()
                .ok_or_else(|| Error::Invalid("missing git stdin".into()))?;
            std::thread::scope(|scope| {
                let writer = scope.spawn(move || stdin.write_all(input));
                let output = child.wait_with_output();
                let written = writer
                    .join()
                    .map_err(|_| Error::Invalid("git stdin writer panicked".into()))?;
                let output = output?;
                if output.status.success() {
                    written?;
                }
                Ok(output)
            })
        } else {
            Ok(child.wait_with_output()?)
        }
    }

    pub(crate) fn checked<S: AsRef<OsStr>>(
        &self,
        path: Option<&Path>,
        args: &[S],
        read_only: bool,
        extra: &[(OsString, OsString)],
        input: Option<&[u8]>,
    ) -> Result<Vec<u8>> {
        let output = self.run(path, args, read_only, extra, input)?;
        check(args, output)
    }
}

pub(crate) fn check<S: AsRef<OsStr>>(args: &[S], output: Output) -> Result<Vec<u8>> {
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(failure(args, &output))
    }
}

pub(crate) fn failure<S: AsRef<OsStr>>(args: &[S], output: &Output) -> Error {
    Error::Command {
        args: args
            .iter()
            .map(|s| s.as_ref().to_string_lossy())
            .collect::<Vec<_>>()
            .join(" "),
        code: output.status.code(),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
    }
}

pub(crate) fn valid_oid(oid: &Oid) -> Result<()> {
    if matches!(oid.0.len(), 40 | 64) && oid.0.bytes().all(|c| c.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(Error::Invalid(format!("invalid object id: {}", oid.0)))
    }
}

pub(crate) fn valid_path(path: &str) -> Result<()> {
    if path.is_empty()
        || path.contains('\0')
        || path.contains('\\')
        || Path::new(path).is_absolute()
        || path
            .split('/')
            .any(|p| p == ".." || p == ".git" || p == "." || p.is_empty())
    {
        Err(Error::Invalid(format!(
            "not a repo-relative file path: {path:?}"
        )))
    } else {
        Ok(())
    }
}

pub(crate) struct TempIndex {
    dir: PathBuf,
    pub path: PathBuf,
}

impl TempIndex {
    pub fn new() -> Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        for _ in 0..100 {
            let dir = std::env::temp_dir().join(format!(
                "brigadier-git-{}-{time}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let builder = fs::DirBuilder::new();
            #[cfg(unix)]
            let builder = {
                use std::os::unix::fs::DirBuilderExt;
                let mut builder = builder;
                builder.mode(0o700);
                builder
            };
            match builder.create(&dir) {
                Ok(()) => {
                    return Ok(Self {
                        path: dir.join("index"),
                        dir,
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Err(Error::Invalid("could not allocate temporary index".into()))
    }

    pub fn env(&self) -> Environment {
        vec![("GIT_INDEX_FILE".into(), self.path.as_os_str().to_owned())]
    }
}

impl Drop for TempIndex {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}
