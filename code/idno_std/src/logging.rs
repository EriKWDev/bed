//! log-crate backend for every reality binary: level-colored stdout lines
//! (bladerend GameLogger lineage) with repo-relative file:line, noisy
//! dependency chatter filtered at the door. Everything printed is also
//! captured in a bounded in-memory ring so the editor can show a log
//! window and raise warning/error notifications.

use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};

pub struct CapturedLog {
    pub sequence: u64,
    pub level: log::Level,
    pub module: String,
    pub file: String,
    pub line: u32,
    pub message: String,
}

const CAPTURE_LIMIT: usize = 2048;

#[derive(Default)]
pub struct LoggingRuntime {
    pub captured: std::sync::Mutex<Vec<CapturedLog>>,
    pub sequence: AtomicU64,
    pub repo_root: std::sync::OnceLock<std::path::PathBuf>,
    pub notification_silenced: std::sync::Mutex<Vec<String>>,
}

pub fn repo_root() -> Option<&'static std::path::Path> {
    crate::process_runtime()
        .logging
        .repo_root
        .get()
        .map(|root| root.as_path())
}

/// Module prefixes whose WARNINGS still log and capture but never raise a
/// notification toast (wgpu/winit/calloop warning chatter etc). Errors
/// always notify. Any code can add to this.
pub fn silence_notifications_from(module_prefix: &str) {
    profiling::function_scope!();
    crate::process_runtime()
        .logging
        .notification_silenced
        .lock()
        .unwrap()
        .push(module_prefix.to_string());
}

pub fn notifications_silenced_for(module: &str) -> bool {
    crate::process_runtime()
        .logging
        .notification_silenced
        .lock()
        .unwrap()
        .iter()
        .any(|prefix| module.starts_with(prefix.as_str()))
}

pub fn captured_sequence() -> u64 {
    crate::process_runtime()
        .logging
        .sequence
        .load(Ordering::Relaxed)
}

pub fn with_captured<R>(f: impl FnOnce(&[CapturedLog]) -> R) -> R {
    f(&crate::process_runtime().logging.captured.lock().unwrap())
}

/// Debug/Info chatter from these dependency roots never reaches stdout or
/// the capture; their warnings and errors still do.
const NOISY_MODULES: &[&str] = &[
    "wgpu_core",
    "wgpu_hal",
    "naga",
    "winit",
    "calloop",
    "smithay",
    "sctk",
    "arboard",
    "polling",
    "zbus",
    "tquic",
    "mio",
    "rustls",
];

struct Logger {
    out: std::io::Stdout,
    repo_root: std::path::PathBuf,
}

impl log::Log for Logger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Debug
    }

    fn log(&self, record: &log::Record) {
        let level = record.level();
        if level == log::Level::Trace {
            return;
        }
        let module = record
            .module_path_static()
            .or_else(|| record.module_path())
            .unwrap_or("");
        if level >= log::Level::Info && NOISY_MODULES.iter().any(|noisy| module.starts_with(noisy))
        {
            return;
        }

        #[rustfmt::skip]
        let level_string = match level {
            log::Level::Error => "\x1b[31mERROR\x1b[0m",
            log::Level::Warn  => "\x1b[93mWARN \x1b[0m",
            log::Level::Info  => "\x1b[32mINFO \x1b[0m",
            log::Level::Debug => "\x1b[34mDEBUG\x1b[0m",
            log::Level::Trace => "\x1b[35mTRACE\x1b[0m",
        };
        let file = record.file_static().or_else(|| record.file()).unwrap_or("");
        let file = std::path::Path::new(file);
        let file = file.strip_prefix(&self.repo_root).unwrap_or(file);
        let line = record.line().unwrap_or(0);
        let message = format!("{}", record.args());

        {
            let mut lock = self.out.lock();
            let _ = writeln!(
                lock,
                "\x1b[90m[ {level_string}\x1b[90m {}:{line} ]\x1b[0m {message}",
                file.display()
            );
        }

        let logging = &crate::process_runtime().logging;
        let sequence = logging.sequence.fetch_add(1, Ordering::Relaxed);
        let mut captured = logging.captured.lock().unwrap();
        if captured.len() >= CAPTURE_LIMIT {
            captured.drain(..CAPTURE_LIMIT / 4);
        }
        captured.push(CapturedLog {
            sequence,
            level,
            module: module.to_string(),
            file: file.display().to_string(),
            line,
            message,
        });
    }

    fn flush(&self) {
        let _ = self.out.lock().flush();
    }
}

pub fn init() {
    profiling::function_scope!();
    log::set_max_level(log::LevelFilter::Debug);
    let mut repo_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    repo_root.pop();
    repo_root.pop();
    let _ = crate::process_runtime()
        .logging
        .repo_root
        .set(repo_root.clone());
    let _ = log::set_boxed_logger(Box::new(Logger {
        out: std::io::stdout(),
        repo_root,
    }));
}
