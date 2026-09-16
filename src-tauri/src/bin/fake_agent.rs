//! A minimal fake ACP agent used by integration tests.
//!
//! Plain `std`, no async: reads newline-delimited JSON-RPC 2.0 frames from
//! stdin and writes them to stdout. It speaks just enough ACP to drive the
//! `acp_flow` integration tests.
//!
//! The agent's behavior is selected by its first positional argument (the
//! "mode"):
//!
//! - *default* (no arg): `session/prompt` → two `session/update` notifications
//!   (`agent_message_chunk`, text `"hello"` then `" world"`, same
//!   `messageId` `"m1"`) then `stopReason: "end_turn"`.
//! - `permission`: `session/prompt` → a `session/update` chunk (`"pre"`)
//!   FIRST, then a `session/request_permission` request (one option, id
//!   `"opt-1"`) and a wait for the client's response. Once the response
//!   arrives, it echoes the outcome as a chunk (`outcome:selected:opt-1` or
//!   `outcome:cancelled`), then the two `"hello"`/`" world"` chunks, then
//!   `end_turn`.
//! - `resume`: `initialize` advertises `loadSession: true`; `session/load`
//!   replays one chunk (`"resumed"`) and then responds, so the client can
//!   verify the `session/load` round-trip.
//! - `hang`: `initialize` is answered, but `session/new` is ignored forever.
//!   The client's establishment timeout must fire instead of waiting on the
//!   agent indefinitely.
//!
//! Wire-format notes: property keys are camelCase (`sessionUpdate`,
//! `agentCapabilities`, `messageId`, `optionId`); discriminator
//! values are snake_case (`agent_message_chunk`, `end_turn`, `allow_once`).

use std::io::{self, BufRead, Write};
use std::process::ExitCode;

/// Session id this agent reports: an explicit `FAKE_SESSION_ID` env
/// override, or the default `fake-session-1` (existing tests rely on it).
fn session_id() -> String {
    std::env::var("FAKE_SESSION_ID").unwrap_or_else(|_| "fake-session-1".to_string())
}

fn main() -> ExitCode {
    let stdin = io::stdin();
    let stdout = io::stdout();

    let mut reader = stdin.lock();
    let mut out = stdout.lock();

    // Mode is the first positional argument (see the module docs).
    let mode = std::env::args().nth(1).unwrap_or_default();

    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
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
                let load_session = mode == "resume";
                let result = serde_json::json!({
                    "protocolVersion": 1,
                    "agentInfo": {
                        "name": "fake-agent",
                        "title": "Fake Agent",
                        "version": "0.1.0",
                    },
                    "agentCapabilities": {
                        "loadSession": load_session,
                    },
                });
                write_result(&mut out, &id, &result);
            }
            "session/load" => {
                // Replay one chunk before the response (the client's restore
                // builder retains notifications that arrive pre-response).
                write_chunk(&mut out, "m1", "resumed");
                let result = serde_json::json!({ "sessionId": session_id() });
                write_result(&mut out, &id, &result);
            }
            "session/new" => {
                // `hang` mode: never answer, to hold the client's
                // session/new request open forever.
                if mode != "hang" {
                    let result = serde_json::json!({ "sessionId": session_id() });
                    write_result(&mut out, &id, &result);
                }
            }
            "session/prompt" => match mode.as_str() {
                "permission" => handle_prompt_permission(&mut reader, &mut out, &id),
                _ => handle_prompt_default(&mut out, &id),
            },
            _ => {
                // Unknown method: reply with a JSON-RPC error if the frame
                // expects a response (has an `id`), otherwise ignore it.
                if let Some(id_value) = id {
                    let error = serde_json::json!({
                        "code": -32601,
                        "message": "method not found",
                    });
                    write_error(&mut out, &Some(id_value), &error);
                }
            }
        }
    }

    ExitCode::SUCCESS
}

/// Default mode: stream two chunks, then end the turn.
fn handle_prompt_default(out: &mut impl Write, prompt_id: &Option<serde_json::Value>) {
    write_chunk(out, "m1", "hello");
    write_chunk(out, "m1", " world");
    write_result(
        out,
        prompt_id,
        &serde_json::json!({ "stopReason": "end_turn" }),
    );
}

/// Permission mode: emit a pre-request chunk, request permission, wait for the
/// client's response, echo the outcome, then stream the two chunks.
fn handle_prompt_permission(
    reader: &mut impl BufRead,
    out: &mut impl Write,
    prompt_id: &Option<serde_json::Value>,
) {
    // A chunk emitted BEFORE the permission request. If the client's event
    // loop is blocked while the permission handler is pending, this chunk
    // would never arrive — so its arrival proves the loop stayed responsive.
    write_chunk(out, "m1", "pre");

    let perm_id = 100;
    let sid = session_id();
    write_request(
        out,
        perm_id,
        "session/request_permission",
        &serde_json::json!({
            "sessionId": sid,
            "toolCall": { "toolCallId": "tc1", "title": "run a tool" },
            "options": [
                { "optionId": "opt-1", "name": "Allow", "kind": "allow_once" }
            ],
        }),
    );

    let Some(resp) = read_response(reader, perm_id) else {
        eprintln!("fake_agent: no response to session/request_permission");
        return;
    };

    let outcome = &resp["result"]["outcome"];
    let outcome_text = if let Some(opt) = outcome.get("optionId").and_then(|o| o.as_str()) {
        format!("selected:{opt}")
    } else {
        "cancelled".to_string()
    };
    write_chunk(out, "m1", &format!("outcome:{outcome_text}"));

    write_chunk(out, "m1", "hello");
    write_chunk(out, "m1", " world");
    write_result(
        out,
        prompt_id,
        &serde_json::json!({ "stopReason": "end_turn" }),
    );
}

// ---------------------------------------------------------------------------
// Frame helpers
// ---------------------------------------------------------------------------

/// Read the next JSON-RPC frame whose `id` matches `expected_id`, skipping any
/// intervening frames (e.g. notifications). Returns `None` on EOF.
fn read_response(reader: &mut impl BufRead, expected_id: i64) -> Option<serde_json::Value> {
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => return None,
            Ok(_) => {}
        }
        if line.trim().is_empty() {
            continue;
        }
        let Ok(frame) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if frame.get("id").and_then(|i| i.as_i64()) == Some(expected_id) {
            return Some(frame);
        }
    }
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

/// Write a JSON-RPC request with a numeric `id`, `method`, and `params`.
fn write_request(w: &mut impl Write, id: i64, method: &str, params: &serde_json::Value) {
    let frame = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    });
    write_frame(w, &frame);
}

/// Write a JSON-RPC `session/update` notification with an agent_message_chunk.
fn write_chunk(w: &mut impl Write, message_id: &str, text: &str) {
    let sid = session_id();
    let frame = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {
            "sessionId": sid,
            "update": {
                "sessionUpdate": "agent_message_chunk",
                "content": { "type": "text", "text": text },
                "messageId": message_id,
            },
        },
    });
    write_frame(w, &frame);
}

/// Serialize `frame` to one line, write it, and flush so the peer sees it
/// immediately (ordering matters: chunks must arrive before the prompt reply).
fn write_frame(w: &mut impl Write, frame: &serde_json::Value) {
    if let Ok(mut s) = serde_json::to_string(frame) {
        s.push('\n');
        let _ = w.write_all(s.as_bytes());
        let _ = w.flush();
    }
}
