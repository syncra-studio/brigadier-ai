//! Dictation (the composer's Dictate button): speech to text on this computer, with
//! whisper.cpp.
//!
//! The speech model is downloaded once, on first use, into the data folder and checked
//! against its published SHA-256. Each dictation then runs in its own short-lived
//! `brigadierd transcribe`: it loads the model while the user speaks, reads the audio from its
//! stdin, prints the text and exits. The daemon only passes audio through, so it never holds
//! the model or its working memory, and nothing loads until someone dictates.
//!
//! Both the download and a transcription are jobs: their requests are answered at once, the
//! blocking work runs off the async runtime (a blocking task, or the helper process), and how
//! they go reaches the app as [`DictationUpdate`]s.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{ExitCode, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use base64::Engine as _;
use brigadier_core::Error;
use brigadier_ipc::protocol::{DictationLanguage, DictationStatus, DictationUpdate};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin};
use tokio::sync::broadcast;

/// whisper.cpp's multilingual "base" model, 5-bit quantized, from its Hugging Face repository at
/// a fixed revision.
const MODEL: &str = "ggml-base-q5_1.bin";
const MODEL_BYTES: u64 = 59_707_625;
const MODEL_SHA256: &str = "422f1ae452ade6f30a004d7e5c6a43195e4433bc370bf23fac9cc591f01a8898";
const MODEL_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-base-q5_1.bin";

/// Audio the engine takes: 16 kHz mono, 16-bit.
const SAMPLE_RATE: u64 = 16_000;
/// The longest dictation, in bytes of audio (10 minutes).
const MAX_AUDIO_BYTES: u64 = 10 * 60 * SAMPLE_RATE * 2;
/// How long a transcription may take once the audio is complete.
const TRANSCRIBE_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// Download progress is reported at most this often.
const PROGRESS_EVERY: Duration = Duration::from_millis(150);
/// Updates a connection may fall behind by before it misses some.
const FEED: usize = 256;

type Result<T> = std::result::Result<T, Error>;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn invalid(message: impl Into<String>) -> Error {
    Error::Invalid(message.into())
}

pub struct Dictation {
    models: PathBuf,
    feed: broadcast::Sender<DictationUpdate>,
    /// The running download's stop flag.
    download: Mutex<Option<Arc<AtomicBool>>>,
    /// Dictations still taking audio, by id.
    live: Mutex<HashMap<String, Live>>,
    next: AtomicU64,
}

/// A dictation taking audio: its helper and the pipe to it.
struct Live {
    child: Child,
    /// Shared with an `append` writing to it, so no lock is held while it waits.
    stdin: Arc<tokio::sync::Mutex<ChildStdin>>,
    received: u64,
}

impl Dictation {
    /// Models are kept in `models`.
    pub fn new(models: PathBuf) -> Self {
        let (feed, _) = broadcast::channel(FEED);
        Self {
            models,
            feed,
            download: Mutex::new(None),
            live: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
        }
    }

    /// Every dictation's and download's updates; a connection forwards the ones it started.
    pub fn subscribe(&self) -> broadcast::Receiver<DictationUpdate> {
        self.feed.subscribe()
    }

    fn model_path(&self) -> PathBuf {
        self.models.join("whisper").join(MODEL)
    }

    /// Checked when its download ended; after that, a file of the right size is taken as it.
    fn installed(&self) -> bool {
        std::fs::metadata(self.model_path()).is_ok_and(|meta| meta.len() == MODEL_BYTES)
    }

    pub fn status(&self) -> DictationStatus {
        DictationStatus {
            available: true,
            model: MODEL.to_owned(),
            model_bytes: MODEL_BYTES,
            installed: self.installed(),
            downloading: lock(&self.download).is_some(),
            languages: languages(),
        }
    }

    /// What dictation is doing now (a dictation, the model's download), said plainly.
    pub fn work(&self) -> Vec<String> {
        let mut work = Vec::new();
        if !lock(&self.live).is_empty() {
            work.push("dictation".to_owned());
        }
        if lock(&self.download).is_some() {
            work.push("downloading the speech model".to_owned());
        }
        work
    }

    /// Starts downloading the model (answered at once: see the module docs). Already there,
    /// it says so; already downloading, nothing changes.
    pub fn download(self: &Arc<Self>) {
        if self.installed() {
            let _ = self.feed.send(DictationUpdate::Downloaded);
            return;
        }
        let stop = {
            let mut download = lock(&self.download);
            if download.is_some() {
                return;
            }
            let stop = Arc::new(AtomicBool::new(false));
            *download = Some(stop.clone());
            stop
        };
        let this = self.clone();
        tokio::task::spawn_blocking(move || {
            let result = fetch(&this.model_path(), &stop, &this.feed);
            *lock(&this.download) = None;
            let update = match result {
                Ok(()) => DictationUpdate::Downloaded,
                Err(_) if stop.load(Ordering::Relaxed) => {
                    DictationUpdate::DownloadStopped { message: None }
                }
                Err(message) => {
                    tracing::warn!(%message, "speech model download failed");
                    DictationUpdate::DownloadStopped {
                        message: Some(message),
                    }
                }
            };
            let _ = this.feed.send(update);
        });
    }

    pub fn cancel_download(&self) {
        if let Some(stop) = lock(&self.download).as_ref() {
            stop.store(true, Ordering::Relaxed);
        }
    }

    /// Starts a dictation's helper, which loads the model while audio arrives, listening for
    /// `language` (empty or unknown: detected) with `words` as a hint. One dictation runs at a
    /// time: a new one ends the one before.
    pub fn start(&self, language: &str, words: &[String]) -> Result<String> {
        if !self.installed() {
            return Err(invalid("the speech model isn't downloaded yet"));
        }
        let exe = std::env::current_exe()
            .map_err(|err| invalid(format!("couldn't find brigadierd: {err}")))?;
        let mut command = tokio::process::Command::new(exe);
        command.arg("transcribe").arg(self.model_path());
        if known_language(language) {
            command.arg("--language").arg(language);
        }
        if let Some(hint) = words_hint(words) {
            command.arg("--words").arg(hint);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|err| invalid(format!("couldn't start the speech engine: {err}")))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| invalid("the speech engine has no input"))?;
        let id = format!("d{}", self.next.fetch_add(1, Ordering::Relaxed));
        let mut live = lock(&self.live);
        // Dropping a helper kills it.
        live.clear();
        live.insert(
            id.clone(),
            Live {
                child,
                stdin: Arc::new(tokio::sync::Mutex::new(stdin)),
                received: 0,
            },
        );
        Ok(id)
    }

    /// Passes a piece of audio (base64 PCM) to the dictation's helper.
    pub async fn append(&self, id: &str, audio: &str) -> Result<()> {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(audio)
            .map_err(|err| invalid(format!("the audio isn't base64: {err}")))?;
        let stdin = {
            let mut live = lock(&self.live);
            let dictation = live
                .get_mut(id)
                .ok_or_else(|| invalid("no such dictation"))?;
            dictation.received += bytes.len() as u64;
            if dictation.received > MAX_AUDIO_BYTES {
                live.remove(id);
                return Err(invalid("a dictation can be at most 10 minutes long"));
            }
            dictation.stdin.clone()
        };
        stdin
            .lock()
            .await
            .write_all(&bytes)
            .await
            .map_err(|err| invalid(format!("the speech engine stopped: {err}")))
    }

    /// Ends the dictation's audio; its text follows as an update.
    pub fn finish(&self, id: &str) -> Result<()> {
        let Live {
            mut child, stdin, ..
        } = lock(&self.live)
            .remove(id)
            .ok_or_else(|| invalid("no such dictation"))?;
        // Closing the pipe (once an `append` still writing lets go) tells the helper the
        // audio is complete.
        drop(stdin);
        let feed = self.feed.clone();
        let id = id.to_owned();
        tokio::spawn(async move {
            let started = Instant::now();
            let update =
                match tokio::time::timeout(TRANSCRIBE_TIMEOUT, read_answer(&mut child)).await {
                    Ok(Ok(text)) => {
                        tracing::info!(
                            dictation = %id,
                            ms = started.elapsed().as_millis() as u64,
                            "dictation transcribed"
                        );
                        DictationUpdate::Transcribed {
                            dictation_id: id,
                            text,
                        }
                    }
                    Ok(Err(message)) => DictationUpdate::Failed {
                        dictation_id: id,
                        message,
                    },
                    Err(_) => DictationUpdate::Failed {
                        dictation_id: id,
                        message: "transcribing took too long".into(),
                    },
                };
            let _ = feed.send(update);
        });
        Ok(())
    }

    /// Drops the dictation: its helper is killed with its audio.
    pub fn cancel(&self, id: &str) {
        lock(&self.live).remove(id);
    }

    /// Drops every dictation and stops the model's download.
    pub fn stop_all(&self) {
        lock(&self.live).clear();
        self.cancel_download();
    }
}

/// The dictations one connection started. It goes with the connection and drops any of them
/// still taking audio, so a client that goes away mid-dictation leaves no helper running and
/// the speech model no longer looks in use.
pub struct Started {
    dictation: Arc<Dictation>,
    ids: HashSet<String>,
}

impl Started {
    pub fn new(dictation: Arc<Dictation>) -> Self {
        Self {
            dictation,
            ids: HashSet::new(),
        }
    }

    pub fn insert(&mut self, id: String) {
        self.ids.insert(id);
    }

    /// Whether the connection started it (it's forgotten either way).
    pub fn remove(&mut self, id: &str) -> bool {
        self.ids.remove(id)
    }
}

impl Drop for Started {
    fn drop(&mut self) {
        // A finished one has already left `live`, so this only drops the ones still taking audio.
        for id in self.ids.drain() {
            self.dictation.cancel(&id);
        }
    }
}

/// The helper's answer: its JSON line on stdout, then its exit.
async fn read_answer(child: &mut Child) -> std::result::Result<String, String> {
    let mut out = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        stdout
            .read_to_string(&mut out)
            .await
            .map_err(|err| format!("couldn't read the speech engine: {err}"))?;
    }
    let status = child
        .wait()
        .await
        .map_err(|err| format!("the speech engine failed: {err}"))?;
    let answer: serde_json::Value = serde_json::from_str(out.trim())
        .map_err(|_| format!("the speech engine stopped ({status})"))?;
    match (answer.get("text"), answer.get("error")) {
        (Some(text), _) => Ok(text.as_str().unwrap_or_default().to_owned()),
        (_, Some(error)) => Err(error
            .as_str()
            .unwrap_or("the speech engine failed")
            .to_owned()),
        _ => Err("the speech engine gave no text".into()),
    }
}

/// Downloads the model to `dest` through `dest.part`, resuming what an earlier try left, and
/// checks its size and SHA-256 before moving it into place.
fn fetch(
    dest: &Path,
    stop: &AtomicBool,
    feed: &broadcast::Sender<DictationUpdate>,
) -> std::result::Result<(), String> {
    let dir = dest.parent().ok_or("the models folder has no parent")?;
    std::fs::create_dir_all(dir)
        .map_err(|err| format!("couldn't make {}: {err}", dir.display()))?;
    let part = dest.with_extension("bin.part");

    let mut hasher = Sha256::new();
    let mut have = match std::fs::File::open(&part) {
        Ok(mut file) => {
            let mut buf = vec![0u8; 1 << 16];
            let mut total = 0u64;
            loop {
                let n = file.read(&mut buf).map_err(|err| err.to_string())?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
                total += n as u64;
            }
            total
        }
        Err(_) => 0,
    };
    if have >= MODEL_BYTES {
        // Complete or overlong leftovers: start over.
        have = 0;
        hasher = Sha256::new();
    }

    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(30)))
        .timeout_recv_response(Some(Duration::from_secs(60)))
        .build()
        .into();
    let mut request = agent.get(MODEL_URL);
    if have > 0 {
        request = request.header("Range", format!("bytes={have}-"));
    }
    let response = request
        .call()
        .map_err(|err| format!("couldn't reach the model's server: {err}"))?;
    let mut file = if have > 0 && response.status().as_u16() == 206 {
        std::fs::OpenOptions::new()
            .append(true)
            .open(&part)
            .map_err(|err| err.to_string())?
    } else {
        // A whole file came back: whatever was kept goes.
        have = 0;
        hasher = Sha256::new();
        std::fs::File::create(&part).map_err(|err| err.to_string())?
    };

    let mut body = response.into_body().into_reader();
    let mut buf = vec![0u8; 1 << 16];
    let mut received = have;
    let mut reported = Instant::now() - PROGRESS_EVERY;
    loop {
        if stop.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let n = body
            .read(&mut buf)
            .map_err(|err| format!("the download broke off: {err}"))?;
        if n == 0 {
            break;
        }
        received += n as u64;
        if received > MODEL_BYTES {
            let _ = std::fs::remove_file(&part);
            return Err("the model's server sent more than expected".into());
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n])
            .map_err(|err| format!("couldn't save the model: {err}"))?;
        if reported.elapsed() >= PROGRESS_EVERY {
            reported = Instant::now();
            let _ = feed.send(DictationUpdate::Download {
                received,
                total: MODEL_BYTES,
            });
        }
    }
    file.sync_all().map_err(|err| err.to_string())?;
    drop(file);

    let digest: String = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if received != MODEL_BYTES || digest != MODEL_SHA256 {
        let _ = std::fs::remove_file(&part);
        return Err("the downloaded model didn't match its checksum".into());
    }
    std::fs::rename(&part, dest).map_err(|err| format!("couldn't keep the model: {err}"))?;
    let _ = feed.send(DictationUpdate::Download {
        received,
        total: MODEL_BYTES,
    });
    Ok(())
}

/// The languages the speech engine knows.
fn languages() -> Vec<DictationLanguage> {
    (0..=whisper_rs::get_lang_max_id())
        .filter_map(|id| {
            Some(DictationLanguage {
                code: whisper_rs::get_lang_str(id)?.to_owned(),
                name: whisper_rs::get_lang_str_full(id)?.to_owned(),
            })
        })
        .collect()
}

/// Whether `code` is the short name of a language the speech engine knows (not "auto", nor a
/// long name such as "english"). Checked before asking it: it can't take a null byte.
fn known_language(code: &str) -> bool {
    !code.is_empty()
        && code.len() <= 8
        && code.bytes().all(|byte| byte.is_ascii_lowercase())
        && whisper_rs::get_lang_id(code).and_then(whisper_rs::get_lang_str) == Some(code)
}

/// Longest word hint passed on: the engine reads the hint as an earlier part of the speech,
/// and takes only so much of it.
const WORDS_HINT_CHARS: usize = 600;

/// The words dictation should recognise, as the speech engine's hint ("Brigadier, Jane Doe"),
/// or None when there are none.
fn words_hint(words: &[String]) -> Option<String> {
    let mut hint = String::new();
    for word in words {
        let word: String = word.chars().filter(|c| !c.is_control()).collect();
        let word = word.trim();
        if word.is_empty() {
            continue;
        }
        let separator = if hint.is_empty() { "" } else { ", " };
        if hint.chars().count() + separator.len() + word.chars().count() > WORDS_HINT_CHARS {
            break;
        }
        hint.push_str(separator);
        hint.push_str(word);
    }
    (!hint.is_empty()).then_some(hint)
}

/// `brigadierd transcribe <model> [--language <code>] [--words <hint>]`: the dictation
/// helper. Reads 16 kHz mono 16-bit PCM from stdin until it closes, loading the model
/// meanwhile, and prints `{"text": …}` (or `{"error": …}`) as one line. Without a language it
/// detects one; the words are a hint of names and terms to expect.
pub fn transcribe_main(args: impl Iterator<Item = std::ffi::OsString>) -> ExitCode {
    let answer = match parse_transcribe(args) {
        Ok(job) => match transcribe(&job) {
            Ok(text) => serde_json::json!({ "text": text }),
            Err(error) => serde_json::json!({ "error": error }),
        },
        Err(error) => serde_json::json!({ "error": error }),
    };
    let ok = answer.get("text").is_some();
    println!("{answer}");
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// What `brigadierd transcribe` was asked to do.
#[derive(Debug, PartialEq)]
struct Transcription {
    model: PathBuf,
    language: Option<String>,
    words: Option<String>,
}

const TRANSCRIBE_USAGE: &str =
    "usage: brigadierd transcribe <model> [--language <code>] [--words <hint>]";

fn parse_transcribe(
    mut args: impl Iterator<Item = std::ffi::OsString>,
) -> std::result::Result<Transcription, String> {
    let model = args.next().ok_or(TRANSCRIBE_USAGE)?;
    let mut job = Transcription {
        model: PathBuf::from(model),
        language: None,
        words: None,
    };
    while let Some(flag) = args.next() {
        let value = args
            .next()
            .and_then(|value| value.into_string().ok())
            .ok_or(TRANSCRIBE_USAGE)?;
        match flag.to_str() {
            Some("--language") if known_language(&value) => job.language = Some(value),
            Some("--language") => return Err(format!("unknown language: {value}")),
            Some("--words") => job.words = words_hint(&[value]),
            _ => return Err(TRANSCRIBE_USAGE.to_owned()),
        }
    }
    Ok(job)
}

fn transcribe(job: &Transcription) -> std::result::Result<String, String> {
    use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

    // whisper.cpp's own logging would go to stderr; nothing reads it.
    whisper_rs::install_logging_hooks();
    // The audio is read while the model loads.
    let audio = std::thread::spawn(|| {
        let mut bytes = Vec::new();
        std::io::stdin().read_to_end(&mut bytes).map(|_| bytes)
    });
    let context = WhisperContext::new_with_params(&job.model, WhisperContextParameters::default())
        .map_err(|err| format!("couldn't load the speech model: {err}"))?;
    let mut state = context
        .create_state()
        .map_err(|err| format!("couldn't start the speech engine: {err}"))?;
    let bytes = audio
        .join()
        .map_err(|_| "reading the audio failed".to_owned())?
        .map_err(|err| format!("reading the audio failed: {err}"))?;
    let samples: Vec<f32> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| f32::from(i16::from_le_bytes(*pair)) / 32768.0)
        .collect();
    // Under a quarter of a second is a tap on the button, not speech.
    if samples.len() < (SAMPLE_RATE / 4) as usize {
        return Ok(String::new());
    }

    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_language(Some(job.language.as_deref().unwrap_or("auto")));
    if let Some(words) = &job.words {
        params.set_initial_prompt(words);
    }
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get().min(8));
    params.set_n_threads(i32::try_from(threads).unwrap_or(4));
    params.set_no_context(true);
    params.set_suppress_blank(true);
    params.set_suppress_nst(true);
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    state
        .full(params, &samples)
        .map_err(|err| format!("transcribing failed: {err}"))?;

    let mut text = String::new();
    for segment in state.as_iter() {
        let piece = segment.to_str_lossy().map_err(|err| err.to_string())?;
        let piece = piece.trim();
        // whisper marks silence and noise as "[BLANK_AUDIO]", "(music)" and the like.
        let marker = (piece.starts_with('[') && piece.ends_with(']'))
            || (piece.starts_with('(') && piece.ends_with(')'));
        if piece.is_empty() || marker {
            continue;
        }
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(piece);
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A dictation taking audio, with `cat` standing in for the helper.
    fn take_audio(dictation: &Dictation, id: &str) {
        let mut child = tokio::process::Command::new("cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        lock(&dictation.live).insert(
            id.to_owned(),
            Live {
                child,
                stdin: Arc::new(tokio::sync::Mutex::new(stdin)),
                received: 0,
            },
        );
    }

    #[tokio::test]
    async fn a_connection_that_goes_away_drops_the_dictation_it_left_running() {
        let dictation = Arc::new(Dictation::new(PathBuf::from("/nonexistent")));
        take_audio(&dictation, "d1");
        let mut theirs = Started::new(dictation.clone());
        theirs.insert("d1".into());
        // Another connection, with a dictation that finished: going away touches nothing else.
        let mut other = Started::new(dictation.clone());
        other.insert("d0".into());
        drop(other);
        assert_eq!(dictation.work(), vec!["dictation".to_owned()]);
        drop(theirs);
        assert!(dictation.work().is_empty());
        assert!(lock(&dictation.live).is_empty());
    }

    fn args(list: &[&str]) -> impl Iterator<Item = std::ffi::OsString> {
        list.iter()
            .map(std::ffi::OsString::from)
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn the_engine_lists_its_languages_by_code_and_name() {
        let languages = languages();
        assert!(languages.len() > 90, "{}", languages.len());
        assert_eq!(languages[0].code, "en");
        assert_eq!(languages[0].name, "english");
        assert!(
            languages
                .iter()
                .any(|lang| lang.code == "de" && lang.name == "german")
        );
    }

    #[test]
    fn only_a_language_the_engine_knows_is_passed_on() {
        assert!(known_language("en"));
        assert!(known_language("yue"));
        for code in ["", "auto", "xx", "EN", "e\0n", "english"] {
            assert!(!known_language(code), "{code:?}");
        }
    }

    #[test]
    fn the_word_hint_joins_words_and_stays_short() {
        let words = ["  Brigadier ", "", "Jane\u{0}Doe", "\n"].map(String::from);
        assert_eq!(words_hint(&words).as_deref(), Some("Brigadier, JaneDoe"));
        assert_eq!(words_hint(&[]), None);
        let many: Vec<String> = (0..500).map(|n| format!("word{n}")).collect();
        let hint = words_hint(&many).unwrap();
        assert!(hint.chars().count() <= WORDS_HINT_CHARS);
        assert!(hint.ends_with(char::is_numeric));
    }

    #[test]
    fn the_helper_reads_its_model_language_and_words() {
        assert_eq!(
            parse_transcribe(args(&["/m.bin"])),
            Ok(Transcription {
                model: PathBuf::from("/m.bin"),
                language: None,
                words: None
            })
        );
        assert_eq!(
            parse_transcribe(args(&[
                "/m.bin",
                "--language",
                "de",
                "--words",
                "Brigadier, Jane"
            ])),
            Ok(Transcription {
                model: PathBuf::from("/m.bin"),
                language: Some("de".into()),
                words: Some("Brigadier, Jane".into())
            })
        );
        assert_eq!(
            parse_transcribe(args(&["/m.bin", "--language", "xx"])),
            Err("unknown language: xx".into())
        );
        assert_eq!(parse_transcribe(args(&[])), Err(TRANSCRIBE_USAGE.into()));
        assert_eq!(
            parse_transcribe(args(&["/m.bin", "--language"])),
            Err(TRANSCRIBE_USAGE.into())
        );
        assert_eq!(
            parse_transcribe(args(&["/m.bin", "--speed", "2"])),
            Err(TRANSCRIBE_USAGE.into())
        );
    }
}
