//! The files a worker hands over with its report, kept in the blob store before its scratch
//! folder and worktree are removed.
//!
//! - **Outputs folder** (`<scratch>/outputs`): every file the worker leaves there is stored
//!   when it reports and again when the task ends, and listed on the task card, where the user
//!   opens it or saves it where they want. Images a Codex worker generates are copied there as
//!   they are made (Codex's own copy goes with its thread).
//! - **Report files**: the files a report names under `artifacts`, or mentions by path in
//!   its text, are stored with it. A report is refused, with what to do instead, when it names
//!   a file that does not exist, or one the worker wrote outside its worktree and scratch folder
//!   (a temp folder), which nobody could read once the task ended.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use brigadier_providers::redact::Redactor;

use super::{SessionManager, blocking, secrets};
use crate::tools::ArtifactInput;
use crate::work::{ArtifactKind, ArtifactRef, Task, TaskKind};
use crate::{Error, Result};

/// The folder in a worker's scratch folder for files meant for the orchestrator or the user.
const OUTPUTS_DIR: &str = "outputs";
/// Largest file stored as an artifact.
const FILE_MAX_BYTES: u64 = 8 * 1024 * 1024;
/// Most files kept from an outputs folder.
const OUTPUTS_MAX: usize = 50;
/// Folders whose files outlive nobody's task: the system's temp folders.
const TEMP_ROOTS: &[&str] = &[
    "/tmp",
    "/private/tmp",
    "/var/folders",
    "/private/var/folders",
];
/// Characters that end a path mentioned in a report.
const PATH_END: &[char] = &[
    '`', '"', '\'', '(', ')', '[', ']', '<', '>', ',', ';', '|', '*',
];

/// A worker's outputs folder.
pub(crate) fn outputs_dir(scratch: &Path) -> PathBuf {
    scratch.join(OUTPUTS_DIR)
}

/// Writes `name` in `dir`, a folder of the worker's scratch it can write in too, without
/// following a link the worker put there: a linked folder is refused, and the file goes to a
/// fresh temporary name first, then takes `name`'s place (a link there is replaced, never
/// written through). On Unix the folder is opened once without following a link, and the file
/// is made and renamed relative to that handle, so a folder swapped for a link meanwhile can't
/// redirect the write.
pub(crate) fn write_unfollowed(dir: &Path, name: &str, bytes: &[u8]) -> crate::Result<()> {
    let invalid = |err: std::io::Error| crate::Error::Invalid(err.to_string());
    std::fs::create_dir_all(dir).map_err(invalid)?;
    #[cfg(unix)]
    {
        use nix::fcntl::{OFlag, open};
        use nix::sys::stat::Mode;
        let flags = OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC;
        let folder = open(dir, flags, Mode::empty()).map_err(|_| {
            crate::Error::Invalid(format!(
                "{} is a link; keep it a plain folder",
                dir.display()
            ))
        })?;
        write_in(&folder, name, bytes)
    }
    #[cfg(not(unix))]
    {
        use std::io::Write as _;
        if !std::fs::symlink_metadata(dir).is_ok_and(|meta| meta.is_dir()) {
            return Err(crate::Error::Invalid(format!(
                "{} is a link; keep it a plain folder",
                dir.display()
            )));
        }
        let temp = dir.join(temp_name(name));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(invalid)?;
        let written = file.write_all(bytes).and_then(|()| file.sync_all());
        drop(file);
        let placed = written.and_then(|()| std::fs::rename(&temp, dir.join(name)));
        if placed.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        placed.map_err(invalid)
    }
}

/// A temporary name for one write of `name`, its own even when two writes of the same name
/// run at once (two `observe`s saving the same file): neither unlinks or publishes the
/// other's file.
fn temp_name(name: &str) -> String {
    format!(".{name}.{}.part", uuid::Uuid::now_v7().simple())
}

/// [`write_unfollowed`]'s write in the folder `folder` already opened: a temporary file made
/// there (never through a link), then renamed to `name` in the same folder.
#[cfg(unix)]
fn write_in(folder: &std::os::fd::OwnedFd, name: &str, bytes: &[u8]) -> crate::Result<()> {
    use std::io::Write as _;

    use nix::fcntl::{OFlag, openat, renameat};
    use nix::sys::stat::Mode;
    use nix::unistd::{UnlinkatFlags, unlinkat};
    let invalid = |err: nix::Error| crate::Error::Invalid(err.to_string());
    let temp = temp_name(name);
    let flags =
        OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC;
    let fd = openat(
        folder,
        temp.as_str(),
        flags,
        Mode::from_bits_truncate(0o644),
    )
    .map_err(invalid)?;
    let mut file = std::fs::File::from(fd);
    let written = file.write_all(bytes).and_then(|()| file.sync_all());
    drop(file);
    let placed = written
        .map_err(|err| crate::Error::Invalid(err.to_string()))
        .and_then(|()| renameat(folder, temp.as_str(), folder, name).map_err(invalid));
    if placed.is_err() {
        let _ = unlinkat(folder, temp.as_str(), UnlinkatFlags::NoRemoveDir);
    }
    placed
}

/// A file to store with a report.
#[derive(Debug)]
struct Found {
    path: PathBuf,
    title: String,
    /// Its path in the outputs folder, for files there.
    output: Option<String>,
}

/// What a report hands over besides its text.
pub(crate) struct ReportFiles {
    pub artifacts: Vec<ArtifactRef>,
    pub outputs: Vec<ArtifactRef>,
}

/// Where a report's files may live, and where they must not.
struct Places {
    /// The scratch folder, as recorded and as resolved.
    scratch: Vec<PathBuf>,
    outputs: PathBuf,
    home: Option<PathBuf>,
    /// Brigadier's data folder (worktrees, scratch folders), as recorded and as resolved: never
    /// a lost place, even when it sits in a temp folder.
    data: Vec<PathBuf>,
    /// Folders whose files are lost when the task ends: temp folders, Codex's generated
    /// images (removed with its thread).
    lost: Vec<PathBuf>,
    /// When the task started: a lost file older than that is not the worker's.
    since: std::time::SystemTime,
}

impl Places {
    fn in_scratch(&self, path: &Path) -> bool {
        self.scratch.iter().any(|root| path.starts_with(root))
    }

    /// `path` as mentioned: absolute, `~/…`, or relative to the scratch folder (`outputs/…`).
    fn resolve(&self, path: &str) -> Option<PathBuf> {
        if let Some(rest) = path.strip_prefix("~/") {
            return Some(self.home.as_ref()?.join(rest));
        }
        let path = Path::new(path);
        Some(if path.is_absolute() {
            path.to_owned()
        } else {
            self.scratch.first()?.join(path)
        })
    }

    /// A path listed under `artifacts`: as [`Places::resolve`], except that a relative path
    /// missing from the scratch folder may name a file in the outputs folder (`notes.md`).
    fn resolve_listed(&self, path: &str) -> Option<PathBuf> {
        let resolved = self.resolve(path)?;
        let in_outputs = self.outputs.join(path);
        Some(
            if Path::new(path).is_relative() && !resolved.exists() && in_outputs.exists() {
                in_outputs
            } else {
                resolved
            },
        )
    }
}

impl SessionManager {
    /// Checks and stores the files a report names or mentions, and the outputs folder.
    /// `Error::Invalid` says what the worker must fix.
    pub(crate) async fn report_files(
        &self,
        task: &Task,
        redactor: Option<Arc<Redactor>>,
        listed: &[ArtifactInput],
        texts: Vec<String>,
    ) -> Result<ReportFiles> {
        let scratch = task
            .workspace
            .as_ref()
            .map(|w| PathBuf::from(&w.scratch))
            .ok_or_else(|| Error::Invalid("the task has no workspace".into()))?;
        let mut places = self.places(&scratch, task.created_at_ms);
        if task.kind == TaskKind::Operate {
            // An operate task changes files through apps: the document it edited and saved is
            // the job's result, kept by the app, not an output lost with a temp folder. Asking
            // for a copy cost a resubmitted report (and a model call) on every document task.
            places.lost.clear();
        }
        let listed = listed.to_vec();
        let outputs = places.outputs.clone();
        let found = blocking(move || {
            collect(&places, &listed, &texts).map_err(|problems| {
                Error::Invalid(format!(
                    "Your report was not accepted:\n- {}\nFix this and call submit_report again. Files for the orchestrator or the user go in your outputs folder ({}); name them under `artifacts`.",
                    problems.join("\n- "),
                    outputs.display()
                ))
            })
        })
        .await?;
        let mut files = ReportFiles {
            artifacts: Vec::new(),
            outputs: Vec::new(),
        };
        for file in found {
            let artifact = self.store_file(&file.path, file.title, &redactor).await?;
            match file.output {
                Some(_) => files.outputs.push(artifact),
                None => files.artifacts.push(artifact),
            }
        }
        Ok(files)
    }

    /// The files in a task's outputs folder as it ends, stored; titles given in the report are
    /// kept. `None` when there is nothing new to record.
    pub(crate) async fn final_outputs(&self, task: &Task) -> Option<Vec<ArtifactRef>> {
        let scratch = PathBuf::from(&task.workspace.as_ref()?.scratch);
        let outputs = outputs_dir(&scratch);
        // As a report's outputs: a file over 8 MB is not read at all.
        let files = blocking(move || {
            let mut files = output_files(&outputs);
            files.retain(|(path, _)| {
                let small = std::fs::metadata(path).is_ok_and(|meta| meta.len() <= FILE_MAX_BYTES);
                if !small {
                    tracing::warn!(path = %path.display(), "an output over 8 MB was not kept");
                }
                small
            });
            Ok(files)
        })
        .await
        .ok()?;
        if files.is_empty() {
            return None;
        }
        let redactor = self.task_redactor(task).await;
        let mut stored = Vec::new();
        for (path, name) in files.into_iter().take(OUTPUTS_MAX) {
            match self.store_file(&path, name, &redactor).await {
                Ok(mut artifact) => {
                    if let Some(known) = task.outputs.iter().find(|o| o.id == artifact.id) {
                        artifact.title.clone_from(&known.title);
                    }
                    stored.push(artifact);
                }
                Err(err) => {
                    tracing::warn!(task = %task.id, path = %path.display(), error = %err, "an output was not kept");
                }
            }
        }
        (stored != task.outputs).then_some(stored)
    }

    /// Copies an image a Codex worker generated into its outputs folder.
    pub(crate) async fn keep_generated_image(&self, outputs: PathBuf, image: PathBuf) {
        let copied = blocking(move || {
            let name = image
                .file_name()
                .ok_or_else(|| Error::Invalid(format!("{} has no file name", image.display())))?;
            let target = outputs.join(name);
            if !target.exists() {
                std::fs::create_dir_all(&outputs).map_err(|err| io_error(&outputs, err))?;
                std::fs::copy(&image, &target).map_err(|err| io_error(&image, err))?;
            }
            Ok(())
        })
        .await;
        if let Err(err) = copied {
            tracing::warn!(error = %err, "a generated image was not copied to the outputs folder");
        }
    }

    /// The redactor for a task's files: its live worker's, or rebuilt from the project's
    /// secret files after a restart.
    pub(super) async fn task_redactor(&self, task: &Task) -> Option<Arc<Redactor>> {
        if let Some(live) = self.existing_task_live(&task.id)
            && let Some(redactor) = live.redactor().await
        {
            return Some(redactor);
        }
        secrets::redactor(self.session_secret_values(&task.conversation_id).await)
    }

    /// Stores a file (text redacted) as an artifact.
    async fn store_file(
        &self,
        path: &Path,
        title: String,
        redactor: &Option<Arc<Redactor>>,
    ) -> Result<ArtifactRef> {
        let file = path.to_owned();
        let bytes =
            blocking(move || std::fs::read(&file).map_err(|err| io_error(&file, err))).await?;
        let bytes = match (std::str::from_utf8(&bytes), redactor) {
            (Ok(text), Some(redactor)) => redactor.redact(text).into_owned().into_bytes(),
            _ => bytes,
        };
        let mime = mime_for(path);
        let size = bytes.len() as u64;
        let hash = self.core.store().blobs().put(bytes).await?;
        Ok(ArtifactRef {
            id: hash.to_string(),
            title,
            kind: if mime.starts_with("image/") {
                ArtifactKind::Screenshot
            } else if mime == "text/markdown" {
                ArtifactKind::Note
            } else {
                ArtifactKind::File
            },
            mime,
            bytes: size,
            file_name: path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned()),
        })
    }

    fn places(&self, scratch: &Path, since_ms: i64) -> Places {
        let env = self.runtime.cli_env();
        let home = env.home();
        let mut lost: Vec<PathBuf> = TEMP_ROOTS.iter().map(PathBuf::from).collect();
        if let Some(tmp) = env.var("TMPDIR").filter(|tmp| !tmp.is_empty()) {
            lost.push(PathBuf::from(tmp));
        }
        let codex_home = match env.var("CODEX_HOME").filter(|dir| !dir.is_empty()) {
            Some(dir) => Some(PathBuf::from(dir)),
            None => home.as_ref().map(|home| home.join(".codex")),
        };
        lost.extend(codex_home.map(|dir| dir.join("generated_images")));
        let with_real = |dir: &Path| {
            let mut dirs = vec![dir.to_owned()];
            dirs.extend(std::fs::canonicalize(dir).ok());
            dirs
        };
        Places {
            scratch: with_real(scratch),
            outputs: outputs_dir(scratch),
            home,
            data: with_real(&self.data_dir),
            lost,
            since: UNIX_EPOCH + Duration::from_millis(u64::try_from(since_ms).unwrap_or(0)),
        }
    }
}

/// The files a report hands over, or what is wrong with it.
fn collect(
    places: &Places,
    listed: &[ArtifactInput],
    texts: &[String],
) -> std::result::Result<Vec<Found>, Vec<String>> {
    let mut found: Vec<Found> = Vec::new();
    let mut seen = HashSet::new();
    let mut problems = Vec::new();
    let mut add = |path: PathBuf, title: String, found: &mut Vec<Found>| {
        if let Ok(real) = std::fs::canonicalize(&path)
            && seen.insert(real.clone())
        {
            found.push(Found {
                path: real,
                title,
                output: None,
            });
        }
    };

    for artifact in listed {
        let Some(path) = places.resolve_listed(&artifact.path) else {
            problems.push(format!("`{}` is not a path you can use", artifact.path));
            continue;
        };
        match std::fs::metadata(&path) {
            Err(_) => problems.push(format!(
                "`{}` (in artifacts) does not exist. Save the file first, or leave it out.",
                artifact.path
            )),
            Ok(meta) if meta.is_dir() => problems.push(format!(
                "`{}` (in artifacts) is a folder: name the files in it.",
                artifact.path
            )),
            Ok(_) if !inside(&places.scratch, &path) => problems.push(format!(
                "`{}` is outside your scratch folder, so nobody can read it after the task ends. Copy it into your outputs folder and name that copy instead.",
                artifact.path
            )),
            Ok(meta) if meta.len() > FILE_MAX_BYTES => problems.push(format!(
                "`{}` is larger than 8 MB; save a smaller file.",
                artifact.path
            )),
            Ok(_) => add(path, artifact.title.clone(), &mut found),
        }
    }

    for text in texts {
        for mention in mentions(text, places) {
            let Some(path) = places.resolve(&mention) else {
                continue;
            };
            let path = normalize(&path);
            if places.in_scratch(&path) {
                match std::fs::metadata(&path) {
                    Err(_) => problems.push(format!(
                        "your report mentions `{mention}`, which does not exist. Write the file, or don't mention it."
                    )),
                    Ok(meta) if meta.is_file() && !inside(&places.scratch, &path) => {
                        problems.push(format!(
                            "`{mention}` links to a file outside your scratch folder, so it is not kept. Copy the file itself into your outputs folder and mention that copy instead."
                        ));
                    }
                    Ok(meta) if meta.is_file() && meta.len() > FILE_MAX_BYTES => {
                        problems.push(format!(
                            "`{mention}` is larger than 8 MB; save a smaller file."
                        ));
                    }
                    Ok(meta) if meta.is_file() => {
                        // Its path in the scratch folder, or in the outputs folder.
                        let relative = places
                            .scratch
                            .iter()
                            .find_map(|root| path.strip_prefix(root).ok())
                            .unwrap_or(&path);
                        let title = relative
                            .strip_prefix(OUTPUTS_DIR)
                            .unwrap_or(relative)
                            .display()
                            .to_string();
                        add(path, title, &mut found);
                    }
                    Ok(_) => {}
                }
            } else if !places.data.iter().any(|root| path.starts_with(root))
                && places.lost.iter().any(|root| path.starts_with(root))
                && std::fs::metadata(&path).is_ok_and(|meta| {
                    // A folder mentioned ("/tmp") is not a file it made.
                    meta.is_file()
                        && meta
                            .modified()
                            .is_ok_and(|modified| modified >= places.since)
                })
            {
                problems.push(format!(
                    "your report points to `{mention}`, which is outside your worktree and scratch folder: it is not kept, and nobody can read it after the task ends. Move it into your outputs folder and mention that path instead."
                ));
            }
        }
    }

    // The outputs folder, whole: files named above keep their titles.
    let outputs = output_files(&places.outputs);
    if outputs.len() > OUTPUTS_MAX {
        problems.push(format!(
            "your outputs folder holds {} files; keep at most {OUTPUTS_MAX} (put the rest in an archive).",
            outputs.len()
        ));
    }
    for (path, name) in outputs {
        match std::fs::metadata(&path) {
            Ok(meta) if meta.len() > FILE_MAX_BYTES => problems.push(format!(
                "`{}` in your outputs folder is larger than 8 MB; save a smaller file.",
                path.display()
            )),
            _ => {}
        }
        match found.iter_mut().find(|file| file.path == path) {
            Some(file) => file.output = Some(name),
            None if seen.insert(path.clone()) => found.push(Found {
                path,
                title: name.clone(),
                output: Some(name),
            }),
            None => {}
        }
    }

    // A path mentioned twice is one problem.
    let mut listed_once = HashSet::new();
    problems.retain(|problem| listed_once.insert(problem.clone()));
    if problems.is_empty() {
        Ok(found)
    } else {
        Err(problems)
    }
}

/// Regular files in an outputs folder (resolved), with their paths inside it, in order.
/// Hidden files and links are left out, and so is the whole folder when the worker replaced
/// it with a link: the daemon reads only what lies inside the scratch folder.
fn output_files(outputs: &Path) -> Vec<(PathBuf, String)> {
    if !std::fs::symlink_metadata(outputs).is_ok_and(|meta| meta.is_dir()) {
        return Vec::new();
    }
    let Ok(root) = std::fs::canonicalize(outputs) else {
        return Vec::new();
    };
    let mut files = Vec::new();
    let mut dirs = vec![root.clone()];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let path = entry.path();
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => dirs.push(path),
                Ok(kind) if kind.is_file() => {
                    let name = path
                        .strip_prefix(&root)
                        .unwrap_or(&path)
                        .display()
                        .to_string();
                    files.push((path, name));
                }
                _ => {}
            }
        }
    }
    files.sort_by(|a, b| a.1.cmp(&b.1));
    files
}

/// Paths mentioned in a report's text that may be files the worker made: absolute ones,
/// `~/…` and `outputs/…`. The scratch folder's path may contain spaces, so it is matched
/// whole first.
fn mentions(text: &str, places: &Places) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = text.to_owned();
    for root in &places.scratch {
        let root = root.display().to_string();
        while let Some(start) = rest.find(&root) {
            let tail = &rest[start + root.len()..];
            let end = tail
                .find(|c: char| c.is_whitespace() || PATH_END.contains(&c))
                .unwrap_or(tail.len());
            found.push(clean(&format!("{root}{}", &tail[..end])));
            rest.replace_range(start..start + root.len() + end, " ");
        }
    }
    for word in rest.split(|c: char| c.is_whitespace() || PATH_END.contains(&c)) {
        if word.starts_with('/') && word.len() > 1
            || word.starts_with("~/")
            || word.starts_with("outputs/")
        {
            found.push(clean(word));
        }
    }
    found.retain(|path| path.len() > 1);
    found
}

/// A mentioned path without trailing punctuation or a `:line` suffix.
fn clean(path: &str) -> String {
    let path = path.trim_end_matches(['.', ':', '!', '?']);
    let name_start = path.rfind('/').map_or(0, |i| i + 1);
    match path[name_start..].find(':') {
        Some(colon)
            if path[name_start + colon + 1..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_digit()) =>
        {
            path[..name_start + colon].to_owned()
        }
        _ => path.to_owned(),
    }
}

/// `path` with `.` and `..` resolved lexically, so a mention cannot climb out of a folder.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// Whether `path` resolves to a place inside one of `roots`.
fn inside(roots: &[PathBuf], path: &Path) -> bool {
    std::fs::canonicalize(path).is_ok_and(|real| roots.iter().any(|root| real.starts_with(root)))
}

pub(crate) fn mime_for(path: &Path) -> String {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        Some("pdf") => "application/pdf",
        Some("md") => "text/markdown",
        Some("json") => "application/json",
        Some("diff" | "patch") => "text/x-diff",
        Some("txt" | "log" | "out") => "text/plain",
        Some("csv") => "text/csv",
        Some("html" | "htm") => "text/html",
        _ => "application/octet-stream",
    }
    .into()
}

fn io_error(path: &Path, err: std::io::Error) -> Error {
    Error::Invalid(format!("{}: {err}", path.display()))
}

/// Where artifacts are copied to be opened with their default app. Emptied when the daemon
/// starts.
fn opened_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("cache").join("open")
}

/// Removes the copies opened in an earlier run.
pub(crate) fn clear_opened(data_dir: &Path) {
    let dir = opened_dir(data_dir);
    if let Err(err) = std::fs::remove_dir_all(&dir)
        && err.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(dir = %dir.display(), error = %err, "could not remove opened artifacts");
    }
}

impl SessionManager {
    /// Writes an artifact to `path`, a file the user picked ("Save to…").
    pub async fn save_artifact(&self, id: String, path: String) -> Result<()> {
        let bytes = self.artifact_bytes(&id).await?;
        let path = PathBuf::from(path);
        if !path.is_absolute() {
            return Err(Error::Invalid(format!(
                "{} is not an absolute path",
                path.display()
            )));
        }
        blocking(move || std::fs::write(&path, bytes).map_err(|err| io_error(&path, err))).await
    }

    /// A copy of an artifact named `file_name`, for the user to open with its default app.
    pub async fn artifact_copy(&self, id: String, file_name: String) -> Result<PathBuf> {
        let bytes = self.artifact_bytes(&id).await?;
        let dir = opened_dir(&self.data_dir).join(&id);
        let path = dir.join(super::conversation::safe_file_name(&file_name));
        blocking(move || {
            std::fs::create_dir_all(&dir).map_err(|err| io_error(&dir, err))?;
            std::fs::write(&path, bytes).map_err(|err| io_error(&path, err))?;
            Ok(path)
        })
        .await
    }

    async fn artifact_bytes(&self, id: &str) -> Result<Vec<u8>> {
        let hash = id
            .parse()
            .map_err(|_| Error::Invalid(format!("{id} is not an artifact id")))?;
        self.core
            .store()
            .blobs()
            .get(hash)
            .await?
            .ok_or_else(|| Error::NotFound(format!("artifact {id}")))
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::write_unfollowed;

    #[test]
    fn a_write_replaces_a_planted_link_and_refuses_a_linked_folder() {
        let root =
            std::env::temp_dir().join(format!("brigadier-unfollowed-{}", uuid::Uuid::now_v7()));
        let outside = root.join("outside.txt");
        let dir = root.join("outputs");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&outside, "keep").unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("live-1.png")).unwrap();
        write_unfollowed(&dir, "live-1.png", b"png").unwrap();
        assert_eq!(
            std::fs::read_to_string(&outside).unwrap(),
            "keep",
            "the link's target is untouched"
        );
        let placed = std::fs::symlink_metadata(dir.join("live-1.png")).unwrap();
        assert!(placed.is_file(), "the link was replaced by the file");
        assert_eq!(std::fs::read(dir.join("live-1.png")).unwrap(), b"png");

        let linked = root.join("linked");
        std::os::unix::fs::symlink(root.join("elsewhere"), &linked).unwrap();
        std::fs::create_dir_all(root.join("elsewhere")).unwrap();
        assert!(write_unfollowed(&linked, "live-1.png", b"png").is_err());
        assert!(
            !root.join("elsewhere/live-1.png").exists(),
            "nothing went through the linked folder"
        );
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            1,
            "no temporary file is left"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// The folder is held open from the start: swapped for a link before the write, it still
    /// gets the file, and the link's target gets nothing (Codex review, phase 6).
    #[test]
    fn a_folder_swapped_for_a_link_after_it_opened_still_gets_the_write() {
        use nix::fcntl::{OFlag, open};
        use nix::sys::stat::Mode;
        let root =
            std::env::temp_dir().join(format!("brigadier-unfollowed-{}", uuid::Uuid::now_v7()));
        let dir = root.join("outputs");
        let elsewhere = root.join("elsewhere");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(&elsewhere).unwrap();
        let flags = OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC;
        let folder = open(&dir, flags, Mode::empty()).unwrap();
        std::fs::rename(&dir, root.join("moved")).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &dir).unwrap();
        super::write_in(&folder, "live-1.png", b"png").unwrap();
        assert_eq!(
            std::fs::read(root.join("moved/live-1.png")).unwrap(),
            b"png"
        );
        assert_eq!(
            std::fs::read_dir(&elsewhere).unwrap().count(),
            0,
            "nothing went through the link"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Writes of the same name at once each succeed, and the file is one of them whole, with
    /// no temporary file left (Codex review, phase 6 verify).
    #[test]
    fn writes_of_the_same_name_at_once_each_land_whole() {
        let root =
            std::env::temp_dir().join(format!("brigadier-unfollowed-{}", uuid::Uuid::now_v7()));
        let dir = root.join("outputs");
        let writes: Vec<Vec<u8>> = (0..8u8).map(|n| vec![n; 512 * 1024]).collect();
        for _ in 0..10 {
            std::thread::scope(|scope| {
                let handles: Vec<_> = writes
                    .iter()
                    .map(|bytes| scope.spawn(|| super::write_unfollowed(&dir, "live-1.png", bytes)))
                    .collect();
                for handle in handles {
                    handle.join().unwrap().unwrap();
                }
            });
            let kept = std::fs::read(dir.join("live-1.png")).unwrap();
            assert!(writes.contains(&kept), "the file is one write, whole");
            let names: Vec<_> = std::fs::read_dir(&dir)
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect();
            assert_eq!(names, vec!["live-1.png"], "no temporary file is left");
        }
        std::fs::remove_dir_all(&root).unwrap();
    }
}
