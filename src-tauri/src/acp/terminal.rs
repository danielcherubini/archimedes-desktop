//! Client-side backend for the ACP v1 `terminal/*` methods.
//!
//! The agent asks the client to create terminals (`terminal/create`), pull
//! their output (`terminal/output`), wait for them to exit
//! (`terminal/wait_for_exit`), kill them (`terminal/kill`), and release them
//! (`terminal/release`). There is **no** `terminal/write` method in ACP v1 —
//! the agent cannot write to a terminal, only observe it.
//!
//! Each terminal is a real PTY spawned via [`portable_pty`]. Output is read on
//! a dedicated (blocking) task, buffered, and:
//!   * served to `terminal/output` requests, and
//!   * emitted live as a `terminal-output` Tauri event
//!     (`{ terminalId, data: base64 }`) so the UI can stream it.
//!
//! Concurrency note: the SDK runs every handler on a single event-loop task.
//! The long-lived work (reading output, waiting for exit) is therefore
//! delegated to background tasks so the event loop is never blocked on a
//! slow child process.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::sync::{Arc, Mutex};

use agent_client_protocol::schema::v1::TerminalExitStatus;
use base64::prelude::BASE64_STANDARD;
use base64::Engine as _;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde_json::json;
use tokio::sync::watch;

use crate::acp::errors::AcpError;
use crate::acp::session::EventSink;

/// Default PTY geometry (120 columns × 30 rows).
const PTY_COLS: u16 = 120;
const PTY_ROWS: u16 = 30;

/// Registry of live PTY-backed terminals, keyed by terminal id.
///
/// Uses a `std::sync::Mutex` because handlers never hold the lock across an
/// `.await` (they snapshot the data they need synchronously).
#[derive(Debug, Default)]
pub struct TerminalManager {
    terminals: Mutex<HashMap<String, TerminalEntry>>,
}

#[derive(Debug)]
struct TerminalEntry {
    /// The full output captured so far.
    output: Arc<std::sync::Mutex<Vec<u8>>>,
    /// Set once the child exits; read non-blockingly by `terminal/output`.
    exit: watch::Sender<Option<TerminalExitStatus>>,
    /// Used by `terminal/kill` to terminate the child.
    killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
}

impl TerminalManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Spawn a PTY running `command args...` and register it.
    ///
    /// Returns the new terminal id.
    pub fn create(
        &self,
        command: &str,
        args: &[String],
        cwd: Option<&Path>,
        sink: &Arc<dyn EventSink>,
    ) -> Result<String, AcpError> {
        let system = native_pty_system();
        let pair = system
            .openpty(PtySize {
                rows: PTY_ROWS,
                cols: PTY_COLS,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| AcpError::Io { detail: format!("openpty failed: {e}") })?;

        let command_line = build_command_line(command, args);

        let mut cmd = if cfg!(unix) {
            let mut c = CommandBuilder::new("sh");
            c.arg("-c");
            c.arg(&command_line);
            c
        } else {
            let mut c = CommandBuilder::new("powershell");
            c.arg("-Command");
            c.arg(&command_line);
            c
        };
        if let Some(cwd) = cwd {
            cmd.cwd(cwd);
        }

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| AcpError::Io { detail: format!("spawn failed: {e}") })?;
        let killer = child.clone_killer();
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| AcpError::Io { detail: format!("clone reader failed: {e}") })?;

        let terminal_id = uuid::Uuid::new_v4().to_string();
        let output = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (exit_tx, _) = watch::channel(None::<TerminalExitStatus>);

        let output_clone = output.clone();
        let exit_tx_clone = exit_tx.clone();
        let sink = sink.clone();
        let tid = terminal_id.clone();

        // Read the PTY to EOF, buffer + emit live output, then wait for the
        // child and record its exit status. All blocking I/O, so it runs on a
        // blocking thread.
        tokio::spawn(async move {
            let _ = tokio::task::spawn_blocking(move || {
                let mut child = child;
                let mut reader = reader;
                let mut buf = [0u8; 8192];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            let chunk = &buf[..n];
                            output_clone.lock().unwrap().extend(chunk);
                            let b64 = BASE64_STANDARD.encode(chunk);
                            sink.emit("terminal-output", json!({ "terminalId": tid, "data": b64 }));
                        }
                        Err(_) => break,
                    }
                }
                let status = match child.wait() {
                    Ok(s) => TerminalExitStatus::new().exit_code(Some(s.exit_code())),
                    Err(_) => TerminalExitStatus::new(),
                };
                let _ = exit_tx_clone.send(Some(status));
            })
            .await;
        });

        self.terminals.lock().unwrap().insert(
            terminal_id.clone(),
            TerminalEntry {
                output,
                exit: exit_tx,
                killer,
            },
        );
        Ok(terminal_id)
    }

    /// Return the buffered output, a truncation flag, and the exit status if
    /// the child has already exited.
    pub fn output(&self, terminal_id: &str) -> Option<(String, bool, Option<TerminalExitStatus>)> {
        let map = self.terminals.lock().unwrap();
        let entry = map.get(terminal_id)?;
        let bytes = entry.output.lock().unwrap().clone();
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let exit = entry.exit.borrow().clone();
        Some((text, false, exit))
    }

    /// Subscribe to the terminal's exit status. The returned receiver resolves
    /// (to `Some`) when the child exits.
    pub fn exit_receiver(
        &self,
        terminal_id: &str,
    ) -> Option<watch::Receiver<Option<TerminalExitStatus>>> {
        let map = self.terminals.lock().unwrap();
        Some(map.get(terminal_id)?.exit.subscribe())
    }

    /// Terminate the child without releasing the terminal.
    pub fn kill(&self, terminal_id: &str) -> bool {
        let mut map = self.terminals.lock().unwrap();
        match map.get_mut(terminal_id) {
            Some(entry) => entry.killer.kill().is_ok(),
            None => false,
        }
    }

    /// Drop a terminal and free its resources.
    pub fn release(&self, terminal_id: &str) -> bool {
        let mut map = self.terminals.lock().unwrap();
        map.remove(terminal_id).is_some()
    }
}

/// Join `command` and its `args` into a single shell command line.
fn build_command_line(command: &str, args: &[String]) -> String {
    let mut line = command.to_string();
    for arg in args {
        line.push(' ');
        line.push_str(arg);
    }
    line
}
