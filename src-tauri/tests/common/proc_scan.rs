//! A fast, test-only process observer: a DIRECT `/proc` scan (no fork+exec).
//!
//! `pgrep -f` is a fork+exec over every process on the system (~45 ms here),
//! while the processes under test live only a few ms — a `pgrep`-based check
//! can miss them, race with reaping, and the fork itself can transiently fail
//! (EAGAIN) under load. This scans `/proc` directly (~1 ms): `read_dir(/proc)`,
//! a `comm` read for every pid (a cheap gate), and a `cmdline` read ONLY when
//! the `comm` gate passes.
//!
//! **Semantics (stateless "is a live process matching right now"):** a process
//! is ALIVE iff its `/proc/<pid>` entry exists AND its `cmdline` is NON-EMPTY
//! AND contains the needle. A zombie (killed but not yet reaped) has an EMPTY
//! `cmdline`, so it reads as GONE — the same as `pgrep` (which skips zombies
//! by default), which is exactly what the process-reap assertions want
//! ("the process is dead", not "the parent has reaped it").

use std::path::Path;

/// A fast, stateless "is a process whose cmdline contains `pattern` alive right
/// now" check. See the module docs for why this is a direct `/proc` scan
/// rather than a `pgrep` fork+exec.
pub struct ProcScan {
    /// The full command line to look for (the unique per-test binary path).
    needle: Vec<u8>,
    /// The first 15 bytes of the executable's file name — a cheap `comm` gate
    /// before the (slower) `cmdline` read.
    comm_prefix: Vec<u8>,
}

impl ProcScan {
    /// Build the scanner for a process whose full command line contains
    /// `pattern` (the unique per-test binary path).
    pub fn new(pattern: &Path) -> Self {
        let needle = pattern.to_string_lossy().as_bytes().to_vec();
        // `comm` is the executable's file name, truncated to 15 bytes — a
        // cheap gate before the (slower) `cmdline` read.
        let comm_full = pattern
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        let comm_prefix = comm_full.as_bytes()[..comm_full.len().min(15)].to_vec();
        Self {
            needle,
            comm_prefix,
        }
    }

    /// True if a LIVE (non-zombie) process whose cmdline contains the needle is
    /// present right now. A zombie's `cmdline` is empty, so it reads as GONE.
    pub fn alive(&self) -> bool {
        self.find_pid().is_some()
    }

    /// The pid of a live process whose cmdline contains the needle, if any.
    /// Returns `None` if the process is a zombie (empty `cmdline`) or reaped
    /// (no `/proc` entry).
    pub fn find_pid(&self) -> Option<u32> {
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::ffi::OsStrExt;
            let mut found = None;
            if let Ok(entries) = std::fs::read_dir("/proc") {
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    let b = name.as_bytes();
                    if b.is_empty() || !b.iter().all(|c| c.is_ascii_digit()) {
                        continue;
                    }
                    let pid: u32 = b.iter().fold(0, |a, c| a * 10 + u32::from(c - b'0'));
                    // Read this pid's `comm` (the executable's file name,
                    // truncated to 15 bytes) — a cheap gate before the slower
                    // `cmdline` read. Allocation-free path (fixed buffer).
                    let mut cbuf = [0u8; 64];
                    let cpath = proc_path(pid, b"comm", &mut cbuf);
                    let Ok(comm) = std::fs::read(cpath) else {
                        continue;
                    };
                    if !comm.starts_with(self.comm_prefix.as_slice()) {
                        continue;
                    }
                    // NUL-separated argv. A zombie's `cmdline` is EMPTY — so a
                    // zombie (killed but not yet reaped) can never match and
                    // reads as GONE (the same as `pgrep`'s default). The full
                    // path is the authoritative check (`comm` is truncated —
                    // another test's copy could share the 15-byte prefix).
                    let mut kbuf = [0u8; 64];
                    let kpath = proc_path(pid, b"cmdline", &mut kbuf);
                    let Ok(cmdline) = std::fs::read(kpath) else {
                        continue;
                    };
                    if cmdline.is_empty() {
                        continue;
                    }
                    if cmdline
                        .windows(self.needle.len())
                        .any(|w| w == self.needle.as_slice())
                    {
                        found = Some(pid);
                        break;
                    }
                }
            }
            found
        }
        #[cfg(not(target_os = "linux"))]
        {
            // No `/proc` off-Linux: fall back to a `pgrep` fork+exec (it works,
            // just slower — the flake was observed on Linux). `pgrep` skips
            // zombies by default, so the "zombie = gone" semantics hold.
            let pattern_str = String::from_utf8_lossy(&self.needle).into_owned();
            let out = std::process::Command::new("pgrep")
                .args(["-f", pattern_str.as_ref()])
                .output()
                .ok()?;
            if !out.status.success() {
                return None;
            }
            String::from_utf8_lossy(&out.stdout)
                .split_whitespace()
                .next()
                .and_then(|p| p.parse().ok())
        }
    }
}

/// Build a `/proc/<pid>/<file>` path into `buf` (allocation-free; the fixed
/// buffer is reused by the caller). `pid` is written in decimal.
fn proc_path<'a>(pid: u32, file: &[u8], buf: &'a mut [u8; 64]) -> &'a str {
    let mut i = 0;
    for c in b"/proc/" {
        buf[i] = *c;
        i += 1;
    }
    let mut tmp = [0u8; 11];
    let mut n = 0;
    let mut v = pid;
    if v == 0 {
        tmp[0] = b'0';
        n = 1;
    } else {
        while v > 0 {
            tmp[n] = b'0' + (v % 10) as u8;
            n += 1;
            v /= 10;
        }
        tmp[..n].reverse();
    }
    for c in &tmp[..n] {
        buf[i] = *c;
        i += 1;
    }
    buf[i] = b'/';
    i += 1;
    for c in file {
        buf[i] = *c;
        i += 1;
    }
    std::str::from_utf8(&buf[..i]).unwrap()
}

/// SIGKILL `pid` (a direct `kill(2)` syscall on Linux — NO fork+exec of the
/// `kill` binary, whose fork can transiently fail (EAGAIN) under load).
/// Off-Linux, fall back to the `kill` binary (the flake was observed on Linux).
pub fn kill_pid(pid: u32) {
    #[cfg(target_os = "linux")]
    {
        unsafe {
            libc::kill(pid as i32, libc::SIGKILL);
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = std::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .status();
    }
}
