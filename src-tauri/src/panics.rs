//! Panics leave a clear trace. The release unwinds (Cargo.toml), so a panic on a background
//! thread ends only that thread or task, and the app goes on; without this, nothing would
//! show it happened (a release has no console). Every panic, on any thread, is appended to
//! `panics.log` in the app's log folder (macOS: ~/Library/Logs/com.kreokit.rec), then
//! Rust's own message goes to stderr as before.

use std::backtrace::Backtrace;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::panic::PanicHookInfo;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

/// The log file, once the app knows its log folder.
static LOG: OnceLock<PathBuf> = OnceLock::new();

/// A longer log is moved to `panics.old.log` first.
const MAX_BYTES: u64 = 1_000_000;

/// Installs the hook (once, first thing in `main`).
pub fn install() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Some(path) = LOG.get() {
            let _ = append(
                path,
                &describe(info),
                &Backtrace::force_capture().to_string(),
            );
        }
        previous(info);
    }));
}

/// From now on panics also go to `panics.log` in `dir`.
pub fn log_to(dir: &Path) {
    if fs::create_dir_all(dir).is_ok() {
        let _ = LOG.set(dir.join("panics.log"));
    }
}

/// "KreoKit Rec 0.1.0: panic on thread 'tokio-rt-worker' at src/x.rs:1:2: message".
fn describe(info: &PanicHookInfo) -> String {
    let thread = std::thread::current();
    let message = info
        .payload()
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| info.payload().downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "(no message)".into());
    let place = info
        .location()
        .map(|l| format!(" at {}:{}:{}", l.file(), l.line(), l.column()))
        .unwrap_or_default();
    format!(
        "KreoKit Rec {}: panic on thread '{}'{place}: {message}",
        env!("CARGO_PKG_VERSION"),
        thread.name().unwrap_or("unnamed")
    )
}

fn append(path: &Path, line: &str, backtrace: &str) -> std::io::Result<()> {
    if fs::metadata(path).is_ok_and(|m| m.len() > MAX_BYTES) {
        let _ = fs::rename(path, path.with_file_name("panics.old.log"));
    }
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "[{secs} s since 1970] {line}\n{backtrace}\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A panic on a background thread is written to the log with its thread, place and
    /// message, and the app (here: the test) goes on.
    #[test]
    fn a_background_panic_is_logged() {
        let dir = std::env::temp_dir().join(format!("kreokit-rec-panics-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        install();
        log_to(&dir);
        let joined = std::thread::Builder::new()
            .name("background-worker".into())
            .spawn(|| panic!("the test's own panic"))
            .unwrap()
            .join();
        assert!(joined.is_err());
        // Another test may have set the log first (it's set once per process).
        let log = fs::read_to_string(LOG.get().unwrap()).unwrap();
        assert!(
            log.contains("panic on thread 'background-worker' at src")
                && log.contains("the test's own panic"),
            "{log}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_long_log_is_moved_aside() {
        let dir =
            std::env::temp_dir().join(format!("kreokit-rec-panics-old-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("panics.log");
        fs::write(&path, vec![b'x'; MAX_BYTES as usize + 1]).unwrap();
        append(&path, "line", "trace").unwrap();
        assert!(fs::read_to_string(&path).unwrap().contains("line\ntrace"));
        assert!(dir.join("panics.old.log").is_file());
        let _ = fs::remove_dir_all(&dir);
    }
}
