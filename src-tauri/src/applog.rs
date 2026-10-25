//! The app's own log: `logs/corelens.log` in the data folder.
//!
//! **What happened, never what was said.** A line names an event — the
//! bridge started, a provider answered in so many milliseconds, a change
//! was pushed to a device — and never carries a prompt, a reply, a
//! command's output, a configuration or a secret. Two nets catch what a
//! careless call site might pass anyway: every line is scrubbed of every
//! secret this process has opened (vault logins as they are opened,
//! provider keys as they are read) and of the shapes a secret takes
//! (private keys, crypt hashes, API keys), before it reaches the file.
//!
//! **Quiet by default.** Info and warnings only. Per-call and per-token
//! lines (`verbose`) are written only while *Verbose AI logging* is on —
//! a setting that turns itself off 24 hours after it was turned on — or
//! when `CORELENS_MCP_DEBUG=1` is set.
//!
//! **Bounded.** 10 MB a file, five files: `corelens.log` and
//! `corelens.log.1` … `.4`, the oldest dropped.
//!
//! A panic is written here too, through the same scrub, and to stderr
//! scrubbed the same way: a panic message built from an error can quote
//! whatever the error quoted.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Mutex, OnceLock, RwLock};

use zeroize::Zeroizing;

/// A file's size before the next line starts a new one.
pub const FILE_LIMIT: u64 = 10 * 1024 * 1024;
/// How many files are kept, the current one included.
pub const FILES_KEPT: usize = 5;
/// How long *Verbose AI logging* stays on after it is turned on.
pub const VERBOSE_FOR_MS: i64 = 24 * 60 * 60 * 1000;
pub const FILE_NAME: &str = "corelens.log";
/// The app setting holding when verbose logging ends (ms since the epoch).
pub const VERBOSE_SETTING: &str = "aiVerboseLoggingUntil";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Warn,
    Info,
    Verbose,
}

impl Level {
    fn tag(self) -> &'static str {
        match self {
            Level::Warn => "WARN",
            Level::Info => "INFO",
            Level::Verbose => "VERB",
        }
    }
}

struct Sink {
    dir: PathBuf,
    file: Option<File>,
    size: u64,
}

fn sink() -> &'static Mutex<Option<Sink>> {
    static S: OnceLock<Mutex<Option<Sink>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(None))
}

fn secrets() -> &'static RwLock<Vec<Zeroizing<String>>> {
    static S: OnceLock<RwLock<Vec<Zeroizing<String>>>> = OnceLock::new();
    S.get_or_init(|| RwLock::new(Vec::new()))
}

static VERBOSE_UNTIL: AtomicI64 = AtomicI64::new(0);

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Starts writing under `dir/logs`, and catches panics from here on.
pub fn init(data_dir: &Path) {
    let dir = data_dir.join("logs");
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut s) = sink().lock() {
        let path = dir.join(FILE_NAME);
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        *s = Some(Sink { file: OpenOptions::new().create(true).append(true).open(&path).ok(), dir, size });
    }
    install_panic_hook();
}

/// Where the log is written, once [`init`] has run.
pub fn dir() -> Option<PathBuf> {
    sink().lock().ok().and_then(|s| s.as_ref().map(|s| s.dir.clone()))
}

/// A secret this process now holds, so that no line can carry it.
pub fn remember_secret(value: &str) {
    if value.trim().chars().count() < 4 {
        return;
    }
    if let Ok(mut s) = secrets().write() {
        if !s.iter().any(|k| k.as_str() == value) {
            s.push(Zeroizing::new(value.to_string()));
            s.sort_by_key(|v| std::cmp::Reverse(v.len()));
        }
    }
}

/// Verbose lines until `until_ms` (0 turns them off).
pub fn set_verbose_until(until_ms: i64) {
    VERBOSE_UNTIL.store(until_ms, Ordering::Relaxed);
}

/// Whether a per-call line would be written now.
pub fn verbose() -> bool {
    std::env::var("CORELENS_MCP_DEBUG").as_deref() == Ok("1") || VERBOSE_UNTIL.load(Ordering::Relaxed) > now_ms()
}

/// `text` with every remembered secret and every secret shape removed.
pub fn scrub(text: &str) -> String {
    let mut out = text.to_string();
    if let Ok(s) = secrets().read() {
        for secret in s.iter() {
            out = out.replace(secret.as_str(), crate::mcp::redact::MARK);
        }
    }
    crate::mcp::redact::shapes_only(&out)
}

pub fn write(level: Level, area: &str, message: &str) {
    if level == Level::Verbose && !verbose() {
        return;
    }
    let line = format!("{} {} [{area}] {}\n", crate::mcp::audit_time(now_ms()), level.tag(), scrub(message).replace('\n', " "));
    let Ok(mut guard) = sink().lock() else { return };
    let Some(s) = guard.as_mut() else { return };
    if s.size + line.len() as u64 > FILE_LIMIT {
        rotate(&s.dir);
        s.file = OpenOptions::new().create(true).append(true).open(s.dir.join(FILE_NAME)).ok();
        s.size = 0;
    }
    if let Some(f) = s.file.as_mut() {
        if f.write_all(line.as_bytes()).is_ok() {
            s.size += line.len() as u64;
        }
    }
}

/// `corelens.log` becomes `.1`, `.1` becomes `.2` … and the last is dropped.
fn rotate(dir: &Path) {
    let name = |i: usize| if i == 0 { dir.join(FILE_NAME) } else { dir.join(format!("{FILE_NAME}.{i}")) };
    let _ = std::fs::remove_file(name(FILES_KEPT - 1));
    for i in (0..FILES_KEPT - 1).rev() {
        let _ = std::fs::rename(name(i), name(i + 1));
    }
}

pub fn info(area: &str, message: &str) {
    write(Level::Info, area, message);
}

pub fn warn(area: &str, message: &str) {
    write(Level::Warn, area, message);
}

/// A per-call line, written only while verbose logging is on. The
/// message is built only then.
pub fn verbose_with(area: &str, message: impl FnOnce() -> String) {
    if verbose() {
        write(Level::Verbose, area, &message());
    }
}

fn install_panic_hook() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        std::panic::set_hook(Box::new(|info| {
            let message = info
                .payload()
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| info.payload().downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "a panic with no message".into());
            let at = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
            let clean = scrub(&message);
            eprintln!("corelens-desktop-ai panicked at {at}: {clean}");
            write(Level::Warn, "panic", &format!("at {at}: {clean}"));
        }));
    });
}

/// Every log file there is, newest first, and their total size.
pub fn files() -> (Vec<PathBuf>, u64) {
    let Some(dir) = dir() else { return (Vec::new(), 0) };
    let mut out = Vec::new();
    let mut total = 0;
    for i in 0..FILES_KEPT {
        let p = if i == 0 { dir.join(FILE_NAME) } else { dir.join(format!("{FILE_NAME}.{i}")) };
        if let Ok(m) = std::fs::metadata(&p) {
            total += m.len();
            out.push(p);
        }
    }
    (out, total)
}

/// The log as Settings shows it: where it is and how big.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogInfo {
    pub folder: String,
    pub files: Vec<(String, u64)>,
    pub total_bytes: u64,
    pub limit_bytes: u64,
    pub verbose_until_ms: i64,
}

#[tauri::command(async)]
pub fn app_log_info() -> LogInfo {
    let (files, total_bytes) = files();
    LogInfo {
        folder: dir().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default(),
        files: files.iter().map(|p| (p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), std::fs::metadata(p).map(|m| m.len()).unwrap_or(0))).collect(),
        total_bytes,
        limit_bytes: FILE_LIMIT * FILES_KEPT as u64,
        verbose_until_ms: VERBOSE_UNTIL.load(Ordering::Relaxed),
    }
}

/// *Verbose AI logging*, on for 24 hours from now or off. Remembered so
/// a restart inside the 24 hours keeps it, and ends by itself after.
#[tauri::command(async)]
pub fn app_log_verbose(state: tauri::State<'_, crate::commands::AppState>, on: bool) -> Result<LogInfo, String> {
    let until = if on { now_ms() + VERBOSE_FOR_MS } else { 0 };
    {
        let conn = state.db.lock().map_err(crate::vault_commands::db_err)?;
        crate::db::set_setting(&conn, VERBOSE_SETTING, if on { Some(until.to_string()) } else { None }.as_deref()).map_err(crate::vault_commands::db_err)?;
    }
    set_verbose_until(until);
    info("log", if on { "verbose AI logging turned on for 24 hours" } else { "verbose AI logging turned off" });
    Ok(app_log_info())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// One process, one log: the tests that write through it take turns
    /// and point it at a folder of their own.
    pub(crate) static TURN: Mutex<()> = Mutex::new(());

    pub(crate) fn into(dir: &Path) {
        let logs = dir.join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        *sink().lock().unwrap() = Some(Sink { file: OpenOptions::new().create(true).append(true).open(logs.join(FILE_NAME)).ok(), dir: logs, size: 0 });
    }

    pub(crate) fn everything_written(dir: &Path) -> String {
        let mut all = String::new();
        for entry in std::fs::read_dir(dir.join("logs")).unwrap().flatten() {
            all.push_str(&std::fs::read_to_string(entry.path()).unwrap_or_default());
        }
        all
    }

    #[test]
    fn a_line_never_carries_a_secret_this_process_holds_or_a_secret_shape() {
        let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        into(tmp.path());
        remember_secret("fixture-remembered-secret-777");
        info("test", "the login fixture-remembered-secret-777 was used");
        warn("test", "key sk-ant-api03-FIXTUREfixtureFIXTUREfixture and sk-proj-FIXTUREfixtureFIXTUREfixture1234 in an error");
        warn("test", "a hash $9$fixturefixturefixture in a config line");
        let all = everything_written(tmp.path());
        for s in ["fixture-remembered-secret-777", "sk-ant-api03-FIXTURE", "sk-proj-FIXTURE", "$9$fixture"] {
            assert!(!all.contains(s), "{s} reached the log: {all}");
        }
        assert_eq!(all.lines().count(), 3, "{all}");
    }

    #[test]
    fn verbose_lines_wait_for_the_setting_and_it_ends_by_itself() {
        let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
        if std::env::var("CORELENS_MCP_DEBUG").as_deref() == Ok("1") {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        into(tmp.path());
        set_verbose_until(0);
        verbose_with("ai", || "per call, off".into());
        set_verbose_until(now_ms() + VERBOSE_FOR_MS);
        verbose_with("ai", || "per call, on".into());
        set_verbose_until(now_ms() - 1);
        verbose_with("ai", || "per call, expired".into());
        let all = everything_written(tmp.path());
        assert!(!all.contains("off") && all.contains("per call, on") && !all.contains("expired"), "{all}");
        set_verbose_until(0);
    }

    #[test]
    fn the_files_rotate_at_their_limit_and_only_five_are_kept() {
        let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        into(tmp.path());
        let line = "x".repeat(1024 * 1024);
        for _ in 0..60 {
            info("bulk", &line);
        }
        let names: Vec<String> = std::fs::read_dir(tmp.path().join("logs")).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names.len(), FILES_KEPT, "{names:?}");
        for e in std::fs::read_dir(tmp.path().join("logs")).unwrap().flatten() {
            assert!(e.metadata().unwrap().len() <= FILE_LIMIT, "{:?} is over the limit", e.path());
        }
    }
}
