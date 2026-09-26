//! The bundled tool-gate extension: pi RPC has no permission protocol, so
//! the de-facto permission model for RPC clients is an in-process
//! extension whose `tool_call` hook calls `ctx.ui.confirm` → pi emits
//! `extension_ui_request` → the desktop renders a prompt and answers
//! `extension_ui_response`.
//!
//! The extension source (a single small TS file) is embedded in the
//! binary (`include_str!`), written to `config_dir/pi-gate/gate.ts` at
//! startup (idempotent), and injected into every agent spawn via
//! `-e <path>` + `PI_ARCHIMEDES_GATE=1`. The extension is SELF-GATED on
//! the env var (its first line), so it is inert outside desktop-spawned
//! sessions — the desktop is the only thing that sets the var.
//!
//! Gated tools (Phase 1): the mutating/privileged built-ins — `bash`,
//! `edit`, `write`. `sudo_exec` is NOT gated here: in a desktop spawn the
//! `tools.ts` override replaces the suite's `sudo_exec`, and the desktop's
//! `sudo_exec` handler is the single confirm (a gate confirm + a desktop
//! confirm = double). Read-only tools (`read`/`find`/`grep`/`ls`) and the
//! suite's own `ask`/`subagent`/`manage_todo_list` are ungated (their own
//! UI, or harmless — `ask` / `manage_todo_list` are overridden by `tools.ts`
//! into desktop delegates, whose handlers own the UI).

/// The embedded extension source (`src-tauri/assets/gate.ts`).
pub const GATE_SOURCE: &str = include_str!("../../assets/gate.ts");

/// Write the embedded gate extension to `config_dir/pi-gate/gate.ts`
/// (0644; idempotent — skip the write when the file already matches).
/// Returns the path (used for the `-e` arg).
pub fn install_gate_extension(config_dir: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    let dir = config_dir.join("pi-gate");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("gate.ts");
    // Idempotent: skip the write when the file already matches (no mtime
    // churn on restart).
    if path.exists() && std::fs::read_to_string(&path).is_ok_and(|c| c == GATE_SOURCE) {
        return Ok(path);
    }
    std::fs::write(&path, GATE_SOURCE)?;
    // 0644 (world-readable, owner-writable — the agent child reads it).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))?;
    }
    Ok(path)
}

/// The spawn-args additions for a gated agent: `["-e", <gate path>]`
/// appended (a `None` gate is a no-op — the args are returned as-is).
pub fn gate_spawn_args(gate_path: Option<&std::path::Path>, args: &[String]) -> Vec<String> {
    match gate_path {
        Some(path) => {
            let mut out = args.to_vec();
            out.push("-e".to_string());
            out.push(path.to_string_lossy().into_owned());
            out
        }
        None => args.to_vec(),
    }
}

/// The spawn-env addition: `PI_ARCHIMEDES_GATE=1` (the extension's
/// self-gate — inert without it).
pub fn gate_env(env: &mut std::collections::BTreeMap<String, String>) {
    env.insert("PI_ARCHIMEDES_GATE".to_string(), "1".to_string());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gate-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn install_gate_extension_writes_file() {
        let dir = temp_dir("write");
        let path = install_gate_extension(&dir).expect("install");
        assert_eq!(path, dir.join("pi-gate").join("gate.ts"));
        assert!(path.exists(), "the gate file is written");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            GATE_SOURCE,
            "the content is the embedded source"
        );
        // 0644
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o644, "the mode is 0644");
        }
        // Idempotent: a second install does not rewrite (mtime unchanged).
        let mtime1 = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let path2 = install_gate_extension(&dir).expect("reinstall");
        let mtime2 = std::fs::metadata(&path2).unwrap().modified().unwrap();
        assert_eq!(mtime1, mtime2, "an idempotent install does not rewrite");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn install_gate_extension_rewrites_a_stale_file() {
        let dir = temp_dir("stale");
        let path = install_gate_extension(&dir).unwrap();
        std::fs::write(&path, "stale content").unwrap();
        let path2 = install_gate_extension(&dir).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path2).unwrap(),
            GATE_SOURCE,
            "a stale file is rewritten"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn gate_spawn_args_appends_e_flag() {
        let p = PathBuf::from("/tmp/pi-gate/gate.ts");
        let args = gate_spawn_args(Some(&p), &["--mode".to_string(), "rpc".to_string()]);
        assert_eq!(
            args,
            vec!["--mode", "rpc", "-e", "/tmp/pi-gate/gate.ts"],
            "the -e flag + path are appended"
        );
        let args = gate_spawn_args(None, &["--mode".to_string()]);
        assert_eq!(args, vec!["--mode"], "a None gate is a no-op");
    }

    #[test]
    fn gate_env_appends_var() {
        let mut env = std::collections::BTreeMap::new();
        env.insert("OTHER".to_string(), "x".to_string());
        gate_env(&mut env);
        assert_eq!(env.get("PI_ARCHIMEDES_GATE"), Some(&"1".to_string()));
        assert_eq!(env.get("OTHER"), Some(&"x".to_string()));
    }
}
