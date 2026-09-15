//! A minimal fake ACP agent used by integration tests.
//!
//! Plain `std`, no async: reads newline-delimited JSON-RPC 2.0 frames from
//! stdin and writes them to stdout. It speaks just enough ACP to drive the
//! `acp_flow` integration test:
//!
//! - `initialize`   → `protocolVersion: 1`, `agentInfo`, `agentCapabilities: { loadSession: false }`
//! - `session/new`  → a fixed `sessionId`
//! - `session/prompt` → two `session/update` notifications (`agent_message_chunk`,
//!   text `"hello"` then `" world"`, same `messageId` `"m1"`) then
//!   `stopReason: "end_turn"`
//!
//! Wire-format notes: property keys are camelCase (`sessionUpdate`,
//! `agentCapabilities`, `messageId`); discriminator values are snake_case
//! (`agent_message_chunk`, `end_turn`).
//!
//! Task 3 will extend this fixture (permission requests, file reads, …).

use std::io::{self, BufRead, Write};
use std::process::ExitCode;

/// The fixed session id this fake agent reports for every `session/new`.
const SESSION_ID: &str = "fake-session-1";

fn main() -> ExitCode {
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(frame) = serde_json::from_str::<serde_json::Value>(&line) else {
            eprintln!("fake_agent: unparseable frame: {line}");
            continue;
        };

        let method = frame.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let id = frame.get("id").cloned();

        match method {
            "initialize" => {
                let result = serde_json::json!({
                    "protocolVersion": 1,
                    "agentInfo": {
                        "name": "fake-agent",
                        "title": "Fake Agent",
                        "version": "0.1.0",
                    },
                    "agentCapabilities": {
                        "loadSession": false,
                    },
                });
                write_result(&mut stdout, &id, &result);
            }
            "session/new" => {
                let result = serde_json::json!({ "sessionId": SESSION_ID });
                write_result(&mut stdout, &id, &result);
            }
            "session/prompt" => {
                write_notification(
                    &mut stdout,
                    &serde_json::json!({
                        "sessionId": SESSION_ID,
                        "update": {
                            "sessionUpdate": "agent_message_chunk",
                            "content": { "type": "text", "text": "hello" },
                            "messageId": "m1",
                        },
                    }),
                );
                write_notification(
                    &mut stdout,
                    &serde_json::json!({
                        "sessionId": SESSION_ID,
                        "update": {
                            "sessionUpdate": "agent_message_chunk",
                            "content": { "type": "text", "text": " world" },
                            "messageId": "m1",
                        },
                    }),
                );
                let result = serde_json::json!({ "stopReason": "end_turn" });
                write_result(&mut stdout, &id, &result);
            }
            _ => {
                // Unknown method: reply with a JSON-RPC error if the frame
                // expects a response (has an `id`), otherwise ignore it.
                if let Some(id_value) = id.clone() {
                    let error = serde_json::json!({
                        "code": -32601,
                        "message": "method not found",
                    });
                    write_error(&mut stdout, &Some(id_value), &error);
                }
            }
        }
    }

    ExitCode::SUCCESS
}

/// Write a JSON-RPC `result` response carrying `id` and `result`.
fn write_result(w: &mut impl Write, id: &Option<serde_json::Value>, result: &serde_json::Value) {
    let frame = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    });
    write_frame(w, &frame);
}

/// Write a JSON-RPC `error` response carrying `id` and `error`.
fn write_error(w: &mut impl Write, id: &Option<serde_json::Value>, error: &serde_json::Value) {
    let frame = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": error,
    });
    write_frame(w, &frame);
}

/// Write a JSON-RPC `session/update` notification with the given params.
fn write_notification(w: &mut impl Write, params: &serde_json::Value) {
    let frame = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": params,
    });
    write_frame(w, &frame);
}

/// Serialize `frame` to one line, write it, and flush so the client sees it
/// immediately (ordering matters: chunks must arrive before the prompt reply).
fn write_frame(w: &mut impl Write, frame: &serde_json::Value) {
    if let Ok(mut s) = serde_json::to_string(frame) {
        s.push('\n');
        let _ = w.write_all(s.as_bytes());
        let _ = w.flush();
    }
}
