//! The crash-logging panic hook (ADR 0025 Task 2 — the shared hook,
//! reused by the Supervisor in Task 6): a Worker panic stays in the
//! Worker (the release profile is `panic = "unwind"` — a Worker
//! process dies, the app lives), and the crash is LOGGED to
//! `<data_dir>/archimedes/crash-<ts>-<tag>.log` (the `dir` =
//! `dirs::data_dir().join("archimedes")` — the same resolution as
//! `lib.rs`'s setup).
//!
//! The tags: the Worker's `"worker"` (the session id is unknown before
//! `Start`; the file name carries the timestamp — the Supervisor's
//! crash attribution uses the exit + the `session-stalled`
//! bookkeeping, NOT the file name); the Supervisor's (Task 6):
//! `"supervisor"`.
//!
//! NOTE (toolchain): the panic info is only reachable from INSIDE a
//! panic hook (this std has no `unwrap_panic_hook_info`), so the
//! writer takes the live `PanicHookInfo` (`write_crash_log_from_info`)
//! and the hook closure is the production entry.

/// Write a crash log to `<dir>/crash-<ts>-<tag>.log` from a live
/// `PanicHookInfo` (a `None` base = no-op, never panics). Content: the
/// panic message + location (the `PanicHookInfo`'s `payload_as_str()`
/// + `location()`) + a captured backtrace.
///
/// The capture uses `std::backtrace::Backtrace::force_capture()`: plain
/// `Backtrace::capture()` is EMPTY unless `RUST_BACKTRACE` is set, but
/// `force_capture` gets the frames regardless (raw frames survive
/// `strip`; the message + location always resolve).
pub fn write_crash_log_from_info(
    dir: &std::path::Path,
    tag: &str,
    info: &std::panic::PanicHookInfo,
) -> Option<std::path::PathBuf> {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let path = dir.join(format!("crash-{ts}-{tag}.log"));
    let location = info
        .location()
        .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
        .unwrap_or_default();
    // `force_capture`: plain `Backtrace::capture()` is empty unless
    // `RUST_BACKTRACE` is set — `force_capture` gets the frames
    // regardless (raw frames survive `strip`).
    let backtrace = std::backtrace::Backtrace::force_capture().to_string();
    let content = format!(
        "panic: {}\nlocation: {location}\n\nbacktrace:\n{backtrace}\n",
        info.payload_as_str().unwrap_or("<non-string payload>")
    );
    std::fs::create_dir_all(dir).ok()?;
    std::fs::write(&path, content).ok()?;
    Some(path)
}

/// The production entry (the hook's half): resolves the dirs home
/// (a `None` base = no-op, never panics) and writes the log from the
/// live `PanicHookInfo`.
pub fn write_crash_log(tag: &str, info: &std::panic::PanicHookInfo) -> Option<std::path::PathBuf> {
    let dir = dirs::data_dir()?.join("archimedes");
    write_crash_log_from_info(&dir, tag, info)
}

/// Install the hook (idempotent — `std::panic::set_hook`): writes the
/// log, then calls the previous hook (captured via `take_hook` — this
/// std has no `default_hook`).
pub fn install_panic_hook(tag: &str) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // Take the currently installed hook (the default on the first
        // install) so it is called AFTER the crash log is written.
        let previous = std::panic::take_hook();
        let tag = tag.to_string();
        std::panic::set_hook(Box::new(move |info| {
            let _ = write_crash_log(&tag, info);
            previous(info);
        }));
    });
}

#[cfg(test)]
mod tests {
    use super::{install_panic_hook, write_crash_log_from_info};
    use std::time::Duration;

    /// A temp dir for the crash logs (unique per test).
    fn temp_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("crashlog-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A panic (the hook installed, writing to a temp dir) writes a
    /// crash log containing the panic message + location, with a
    /// NON-EMPTY backtrace section (the `force_capture` behavior —
    /// frames even without `RUST_BACKTRACE`).
    #[test]
    fn the_panic_hook_writes_a_crash_log_with_location_and_a_nonempty_backtrace() {
        let dir = temp_dir();
        let previous = std::panic::take_hook();
        let dir_hook = dir.clone();
        std::panic::set_hook(Box::new(move |info| {
            let _ = write_crash_log_from_info(&dir_hook, "worker", info);
            // The log is the primary output — the previous hook is NOT
            // called here to keep the test output clean.
        }));
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        std::thread::spawn(move || {
            let _ = tx.send(());
            panic!("test crash payload");
        });
        rx.recv().expect("the thread signalled before panicking");
        // The hook runs (synchronously) at the panic's start — the file
        // appears once the hook's write lands (a bounded retry).
        let mut path = None;
        for _ in 0..100 {
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.starts_with("crash-") && name.ends_with("worker.log") {
                        path = Some(entry.path());
                        break;
                    }
                }
            }
            if path.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        // Restore the previous hook (the hook is process-global).
        std::panic::set_hook(previous);
        let path = path.expect("the crash log file appears");
        let content = std::fs::read_to_string(&path).expect("the crash log is readable");
        assert!(
            content.contains("test crash payload"),
            "the panic message is in the log:\n{content}"
        );
        assert!(
            content.contains("location:"),
            "the panic location is in the log:\n{content}"
        );
        assert!(
            content.contains(".rs:"),
            "the location is a real file:line:column:\n{content}"
        );
        // The backtrace section is NON-EMPTY (the `force_capture`
        // behavior — a plain `capture()` would be empty without
        // `RUST_BACKTRACE`).
        let backtrace = content
            .split("backtrace:\n")
            .nth(1)
            .expect("the backtrace section is present");
        assert!(
            !backtrace.trim().is_empty(),
            "force_capture produces frames even without RUST_BACKTRACE"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `install_panic_hook` is idempotent (a double install is safe —
    /// the `Once` guard; the second call is a no-op).
    #[test]
    fn install_panic_hook_is_idempotent() {
        install_panic_hook("worker");
        install_panic_hook("worker");
    }
}
