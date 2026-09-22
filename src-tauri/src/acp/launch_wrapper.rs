//! Per-dispatch pi launch wrapper (ADR 0005).
//!
//! A subagent session must start its pi with a per-dispatch configuration
//! (system prompt, model, tools, thinking, `--no-session`), but `pi-acp`
//! has no CLI surface for any of it — it spawns `pi --mode rpc --no-themes`
//! and only honors `PI_ACP_PI_COMMAND`. So each dispatch gets a small
//! wrapper script (`wrapper-<uuid>.sh`, 0755 — `wrapper-<uuid>.cmd` on
//! Windows) written into the per-spawn 0700 bridge dir; the session exports
//! it as `PI_ACP_PI_COMMAND` and unlinks it at teardown.
//!
//! `build_wrapper_script` is pure and deterministic (no fs, no uuid) — the
//! uuid only appears in `write_wrapper`'s filename.
//!
//! Linux is the platform where the bridge is fully implemented (macOS /
//! Windows bridge listeners are no-ops and fall back to the suite's fork
//! path), so the wrapper is effectively Linux-only in v1. The Windows
//! variant (a `.cmd` file, `@echo off` + `pi <args> %*`) is kept for
//! compilation completeness, with NO quoting engine.

use std::path::{Path, PathBuf};

/// Per-dispatch pi configuration for a subagent session.
pub struct LaunchConfig {
    /// The agent file body (named agents); `None` for config-less dispatch.
    pub system_prompt: Option<String>,
    /// Resolved model ("provider/id" or "provider/id:<thinking>"); `None` = pi default.
    pub model: Option<String>,
    /// Explicit thinking level from the agent file; `None` = pi's own resolution.
    pub thinking: Option<String>,
    /// Tool allowlist (named agents with `tools`); `None` → `--exclude-tools subagent`.
    pub tools: Option<Vec<String>>,
}

/// Quote `s` for a POSIX shell: single-quoted, with embedded single quotes
/// escaped as `'\''` (close quote, backslash-escaped quote, open quote).
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Collect the config flags for `exec pi <flags> ...` (no leading space;
/// the caller separates flags with single spaces). `quoted` selects the
/// POSIX quoting engine; the Windows variant passes values bare (no quoting
/// engine — the suite's fork path covers Windows in v1).
fn config_flags(config: &LaunchConfig, quoted: bool) -> String {
    let mut flags = String::new();
    if let Some(sp) = &config.system_prompt {
        flags.push_str(" --system-prompt ");
        flags.push_str(&if quoted { shell_quote(sp) } else { sp.clone() });
    }
    if let Some(model) = &config.model {
        flags.push_str(" --model ");
        flags.push_str(&if quoted {
            shell_quote(model)
        } else {
            model.clone()
        });
    }
    if let Some(thinking) = &config.thinking {
        flags.push_str(" --thinking ");
        flags.push_str(&if quoted {
            shell_quote(thinking)
        } else {
            thinking.clone()
        });
    }
    match &config.tools {
        Some(tools) => {
            let joined = tools.join(",");
            flags.push_str(" --tools ");
            flags.push_str(&if quoted { shell_quote(&joined) } else { joined });
        }
        None => flags.push_str(" --exclude-tools subagent"),
    }
    flags.push_str(" --no-session");
    flags
}

/// Pure: build the wrapper script text. `pi_command` is the resolved pi
/// binary (usually "pi"). The script execs pi with the config flags and
/// forwards pi-acp's own args (`--mode rpc --no-themes`) via "$@".
pub fn build_wrapper_script(config: &LaunchConfig, pi_command: &str) -> String {
    if cfg!(windows) {
        // Windows variant: a `.cmd` file (no quoting engine — the suite's
        // fork path covers Windows in v1).
        format!(
            "@echo off\r\n{} {} %*\r\n",
            pi_command,
            config_flags(config, false)
        )
    } else {
        format!(
            "#!/bin/sh\nexec {}{} \"$@\"\n",
            pi_command,
            config_flags(config, true)
        )
    }
}

/// Write the wrapper into `dir` (the per-spawn 0700 bridge dir) as
/// `wrapper-<uuid>.sh` (0755; `wrapper-<uuid>.cmd` on Windows — matching
/// the `.cmd` content `build_wrapper_script` emits there) and return its
/// path. `dir` must exist.
pub fn write_wrapper(
    dir: &Path,
    config: &LaunchConfig,
    pi_command: &str,
) -> std::io::Result<PathBuf> {
    // The extension matches the content (`build_wrapper_script` emits a
    // `.cmd` script on Windows): a mismatched extension makes the wrapper
    // unspawnable (`bridge::available()` is `true` on Windows, so the
    // moment anything dispatches there, the `.sh` name would break it).
    let ext = if cfg!(windows) { ".cmd" } else { ".sh" };
    let path = dir.join(format!("wrapper-{}{ext}", uuid::Uuid::new_v4()));
    let script = build_wrapper_script(config, pi_command);
    // A write failure must leave NO partial file behind (a mid-write
    // failure — ENOSPC/EDQUOT — leaves a truncated, unspawnable script;
    // a failure at open leaves nothing). Best-effort unlink of our own
    // path, then propagate the ORIGINAL error (the cleanup result is not
    // what the caller needs).
    if let Err(e) = std::fs::write(&path, script) {
        let _ = std::fs::remove_file(&path);
        return Err(e);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // A chmod failure leaves an unspawnable (0644) wrapper — unlink it
        // the same way (best-effort, original error propagates).
        if let Err(e) = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)) {
            let _ = std::fs::remove_file(&path);
            return Err(e);
        }
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (1) Named agent with tools: every flag present, pi-acp's args forwarded.
    #[test]
    fn named_agent_with_tools_emits_all_flags() {
        let script = build_wrapper_script(
            &LaunchConfig {
                system_prompt: Some("You are a careful reviewer.".into()),
                model: Some("anthropic/claude-sonnet-4-5".into()),
                thinking: Some("high".into()),
                tools: Some(vec!["read".into(), "bash".into()]),
            },
            "pi",
        );
        assert!(script.contains("--system-prompt 'You are a careful reviewer.'"));
        assert!(script.contains("--model 'anthropic/claude-sonnet-4-5'"));
        assert!(script.contains("--thinking 'high'"));
        assert!(script.contains("--tools 'read,bash'"));
        assert!(script.contains("--no-session"));
        assert!(script.contains("\"$@\""));
        assert!(script.contains("exec pi "));
    }

    /// (2) Named agent without tools: `--exclude-tools subagent`, no `--tools`.
    #[test]
    fn named_agent_without_tools_excludes_subagent() {
        let script = build_wrapper_script(
            &LaunchConfig {
                system_prompt: Some("body".into()),
                model: None,
                thinking: None,
                tools: None,
            },
            "pi",
        );
        assert!(script.contains("--exclude-tools subagent"));
        assert!(!script.contains("--tools "));
        assert!(script.contains("--no-session"));
    }

    /// (3) Config-less dispatch (all `None`): only `--exclude-tools subagent --no-session "$@"`.
    #[test]
    fn config_less_dispatch_is_minimal() {
        let script = build_wrapper_script(
            &LaunchConfig {
                system_prompt: None,
                model: None,
                thinking: None,
                tools: None,
            },
            "pi",
        );
        assert!(script.contains("exec pi --exclude-tools subagent --no-session \"$@\""));
        assert!(!script.contains("--system-prompt"));
        assert!(!script.contains("--model"));
        assert!(!script.contains("--thinking"));
        assert!(!script.contains("--tools"));
    }

    /// (4) `shell_quote` escapes embedded single quotes: `a'b` → `'a'\''b'`.
    #[test]
    fn shell_quote_escapes_embedded_single_quotes() {
        assert_eq!(shell_quote("a'b"), "'a'\\''b'");
        assert_eq!(shell_quote("plain"), "'plain'");
        assert_eq!(shell_quote(""), "''");
    }

    /// (5) `write_wrapper` writes an executable (0755) script with the
    /// PLATFORM-CORRECT extension (`.sh` on Unix, `.cmd` on Windows —
    /// matching the content `build_wrapper_script` emits); the path exists.
    #[test]
    fn write_wrapper_writes_executable_script() {
        let dir =
            std::env::temp_dir().join(format!("launch-wrapper-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = write_wrapper(
            &dir,
            &LaunchConfig {
                system_prompt: None,
                model: None,
                thinking: None,
                tools: None,
            },
            "pi",
        )
        .unwrap();
        assert!(path.exists());
        assert_eq!(path.parent().unwrap(), dir.as_path());
        assert!(path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("wrapper-"));
        let expected_ext = if cfg!(windows) { ".cmd" } else { ".sh" };
        assert!(
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .ends_with(expected_ext),
            "the extension must match the platform (the content is a `{expected_ext}` script)"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o755, "wrapper must be 0755");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// (6) `write_wrapper` propagates a write failure (no panic) and leaves
    /// NO wrapper garbage behind: `parent` is a regular FILE, so
    /// `parent/sub/…` cannot be created (ENOTDIR — deterministic even as
    /// root, unlike a chmod-555 directory, which root bypasses). A real
    /// mid-write failure (ENOSPC/EDQUOT — the case that leaves a PARTIAL
    /// file) cannot be forced portably in a unit test, so this pins the
    /// observable contract (error propagates, no garbage) and exercises the
    /// cleanup branch (a no-op here, since the file was never created; the
    /// same branch unlinks a partial file when a real mid-write fails).
    #[test]
    fn write_wrapper_write_failure_leaves_no_wrapper_file() {
        let tmp =
            std::env::temp_dir().join(format!("launch-wrapper-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).unwrap();
        let parent = tmp.join("parent");
        std::fs::write(&parent, b"x").unwrap(); // a FILE, not a directory
        let dir = parent.join("sub"); // ENOTDIR: cannot be created
        let result = write_wrapper(
            &dir,
            &LaunchConfig {
                system_prompt: None,
                model: None,
                thinking: None,
                tools: None,
            },
            "pi",
        );
        assert!(
            result.is_err(),
            "the write failure must propagate (no panic)"
        );
        let garbage: Vec<String> = std::fs::read_dir(&tmp)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("wrapper-"))
            .collect();
        assert!(
            garbage.is_empty(),
            "no wrapper file may be left: {garbage:?}"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
