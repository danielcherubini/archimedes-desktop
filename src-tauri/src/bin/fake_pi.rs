//! A deterministic pi RPC test double (no LLM, no sleeps) for the `PiRpc`
//! unit tests and the session-driver integration tests.
//!
//! Wire behavior mirrors `@earendil-works/pi-coding-agent@0.87.1`
//! `dist/modes/rpc/rpc-mode.js`: one JSON object per line, `response` lines
//! echo the command's `id` (`rpc-mode.js:293`), the `prompt` response is
//! emitted after preflight (start of turn) while the turn's events stream
//! after it, and closing stdin exits 0.
//!
//! **Caveat (documented in the plan):** `fake_pi` is written by the same
//! executor from the same field-name table as `PiRpc`, so the round-trip
//! tests prove serde self-consistency but CANNOT catch a systematic casing
//! error — the `.d.ts` files are the authority, and the real-`pi` smoke test
//! (Task 6) is the true wire check.

use std::io::{BufRead, Write};

/// Mode-independent responses (always available regardless of env — the
/// establisher always calls `get_state`, and `FAKE_PI_PROMPT`-driven tests
/// assert on this data). **These are the `data` payloads only — the full
/// response line (which MUST echo the command's `id` for correlation) is
/// built in the match arms below.**
const GET_STATE_DATA: &str = r#"{"sessionId":"fake-pi-1","sessionFile":"/tmp/fake-pi-session.jsonl","model":{"provider":"fake","id":"fake-model","name":"Fake Model","contextWindow":100000,"cost":{}},"thinkingLevel":"off","isStreaming":false,"steeringMode":"all","followUpMode":"all","autoCompactionEnabled":true,"messageCount":0,"pendingMessageCount":0}"#;

const GET_AVAILABLE_MODELS_DATA: &str = r#"{"models":[{"provider":"fake","id":"fake-model","name":"Fake Model","contextWindow":100000,"cost":{}},{"provider":"fake","id":"fake-model-2","name":"Fake Model 2","contextWindow":200000,"cost":{}}]}"#;

const GET_AVAILABLE_THINKING_LEVELS_DATA: &str = r#"{"levels":["off","low","medium"]}"#;

/// The `get_messages` payload (the `data` object — the response line is
/// built with the `id` in the match arm).
const GET_MESSAGES_DATA: &str = r#"{"messages":[{"role":"user","content":[{"type":"text","text":"hello"}],"timestamp":"2026-09-24T00:00:00.000Z"},{"role":"assistant","content":[{"type":"text","text":"world"}],"api":"fake","provider":"fake","model":"fake-model","usage":{"input":1,"output":1,"cacheRead":0,"cacheWrite":0,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}},"stopReason":"stop","timestamp":"2026-09-24T00:00:01.000Z"}]}"#;

/// The `FAKE_PI_PROMPT` / `FAKE_PI_GATE` turn sequence.
///
/// Order (mirrors the real stream + `rpc-mode.js` preflight semantics):
/// the `prompt` response FIRST (after preflight, echoing the command `id`),
/// then a USER `message_start` (the wire's `message_start` carries any
/// `AgentMessage` — the first one in a turn is the user message; this
/// ordering is what pins the `PiRpc` consumer's `messageId` role rule), then
/// the assistant `message_start`, two `message_update` `text_delta`s ("Hel",
/// "lo"), `message_end` (the full text "Hello"), `agent_end`, `agent_settled`.
const USER_MESSAGE_START: &str = r#"{"type":"message_start","message":{"role":"user","content":[{"type":"text","text":"hi"}],"timestamp":"2026-09-24T00:00:02.000Z"}}"#;
const ASSISTANT_MESSAGE_START: &str = r#"{"type":"message_start","message":{"role":"assistant","content":[],"api":"fake","provider":"fake","model":"fake-model","usage":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}},"timestamp":"2026-09-24T00:00:03.000Z"}}"#;
const TEXT_DELTA_1: &str = r#"{"type":"message_update","usage":__USAGE__,"assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"Hel"}}"#;
const TEXT_DELTA_2: &str = r#"{"type":"message_update","usage":__USAGE__,"assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"lo"}}"#;
const MESSAGE_END: &str = r#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"Hello"}],"api":"fake","provider":"fake","model":"fake-model","usage":{"input":1,"output":1,"cacheRead":0,"cacheWrite":0,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}},"stopReason":"stop","timestamp":"2026-09-24T00:00:04.000Z"}}"#;
const AGENT_END: &str = r#"{"type":"agent_end","messages":[],"willRetry":false}"#;
const AGENT_SETTLED: &str = r#"{"type":"agent_settled"}"#;

const USAGE: &str = r#"{"input":1,"output":1,"cacheRead":0,"cacheWrite":0,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}}"#;

/// The `FAKE_PI_GATE` dialog (the `tool_call` → `ctx.ui.confirm` flow the
/// bundled gate extension produces).
const GATE_REQUEST: &str = r#"{"type":"extension_ui_request","id":"gate-1","method":"confirm","title":"Allow bash?","message":"ls -la"}"#;

/// The `FAKE_PI_UNKNOWN` event (an event type this `PiRpc` version doesn't
/// know — must be received as `RpcEvent::Unknown`, never an error).
const UNKNOWN_EVENT: &str = r#"{"type":"brand_new_event","foo":1}"#;

fn is_set(var: &str) -> bool {
    matches!(std::env::var(var).as_deref(), Ok("1"))
}

fn main() {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());

    // HANG mode: never respond to ANY command (notably `get_state`) — for
    // the rewritten `establishment_times_out_when_the_agent_hangs` test.
    // Consume stdin until EOF so the process stays alive while the (real)
    // client waits, then exit 0.
    if is_set("FAKE_PI_HANG") {
        let lock = stdin.lock();
        for _ in lock.lines() {
            // drain
        }
        return;
    }

    let lock = stdin.lock();
    for line in lock.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break, // stdin closed → exit 0
        };
        let v: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let Some(t) = v.get("type").and_then(|t| t.as_str()) else {
            continue;
        };
        let Some(id) = v.get("id").and_then(|i| i.as_str()) else {
            continue;
        };

        // The client's answer to a dialog (no response of its own — the
        // agent-side correlation is by the request's `id`).
        if t == "extension_ui_response" {
            if is_set("FAKE_PI_GATE") {
                emit_turn(&mut out);
            }
            continue;
        }

        match t {
            "prompt" => {
                // The `prompt` response comes after preflight (start of turn),
                // echoing the command `id`.
                write_line(
                    &mut out,
                    &format!(
                        r#"{{"type":"response","id":"{id}","command":"prompt","success":true}}"#,
                    ),
                );
                if is_set("FAKE_PI_GATE") {
                    // The dialog arrives with the turn's events (the gate
                    // extension fires `tool_call` mid-turn); the turn
                    // sequence follows the client's answer (below) — NOT
                    // here (the agent is blocked awaiting the dialog).
                    write_line(&mut out, GATE_REQUEST);
                } else {
                    if is_set("FAKE_PI_UNKNOWN") {
                        // An unknown event type before settling (permissive
                        // deserialization check).
                        write_line(&mut out, UNKNOWN_EVENT);
                    }
                    // The turn's event sequence (the fake "LLM" output).
                    emit_turn(&mut out);
                }
            }
            "get_state" => write_line(&mut out, &response_line(id, "get_state", GET_STATE_DATA)),
            "get_available_models" => write_line(
                &mut out,
                &response_line(id, "get_available_models", GET_AVAILABLE_MODELS_DATA),
            ),
            "get_available_thinking_levels" => write_line(
                &mut out,
                &response_line(
                    id,
                    "get_available_thinking_levels",
                    GET_AVAILABLE_THINKING_LEVELS_DATA,
                ),
            ),
            "get_messages" => write_line(
                &mut out,
                &response_line(id, "get_messages", GET_MESSAGES_DATA),
            ),
            // Echo the REQUESTED provider/modelId in a `Model` object (so a
            // `set_config_option` round-trip can assert `currentValue` == the
            // value it sent — NOT a static model).
            "set_model" => {
                let data = v
                    .get("provider")
                    .and_then(|p| p.as_str())
                    .zip(
                        v.get("modelId")
                            .and_then(|m| m.as_str()),
                    )
                    .map(|(provider, model_id)| {
                        format!(
                            r#"{{"type":"response","id":"{id}","command":"set_model","success":true,"data":{{"provider":"{provider}","id":"{model_id}","name":"Fake Model","contextWindow":100000,"cost":{{}}}}}}"#
                        )
                    })
                    .unwrap_or_else(|| {
                        format!(r#"{{"type":"response","id":"{id}","command":"set_model","success":false,"error":"missing provider/modelId"}}"#)
                    });
                write_line(&mut out, &data);
            }
            "set_thinking_level" => write_line(
                &mut out,
                &format!(
                    r#"{{"type":"response","id":"{id}","command":"set_thinking_level","success":true}}"#
                ),
            ),
            "abort" => {
                // An aborted turn settles (the client resolves the turn on
                // `agent_settled`).
                write_line(&mut out, AGENT_SETTLED);
            }
            "__fail" => write_line(
                &mut out,
                &format!(
                    r#"{{"type":"response","id":"{id}","command":"__fail","success":false,"error":"boom"}}"#
                ),
            ),
            _ => {
                // Any other command: success, no data (the double is
                // permissive, like the real agent's unknown-command path).
                write_line(
                    &mut out,
                    &format!(r#"{{"type":"response","id":"{id}","command":"{t}","success":true}}"#),
                );
            }
        }
        out.flush().expect("flush");
    }
    // stdin closed → exit 0 (clean shutdown, mirroring pi's `onInputEnd`).
}

/// The turn's event sequence (the fake "LLM" output — no `prompt`
/// response: that was already emitted in the `prompt` match arm, mirroring
/// the real agent's preflight-then-events ordering).
fn emit_turn(out: &mut std::io::BufWriter<std::io::StdoutLock>) {
    write_line(out, USER_MESSAGE_START);
    write_line(out, ASSISTANT_MESSAGE_START);
    write_line(out, &TEXT_DELTA_1.replace("__USAGE__", USAGE));
    write_line(out, &TEXT_DELTA_2.replace("__USAGE__", USAGE));
    write_line(out, MESSAGE_END);
    write_line(out, AGENT_END);
    write_line(out, AGENT_SETTLED);
    out.flush().expect("flush");
}

fn write_line(out: &mut std::io::BufWriter<std::io::StdoutLock>, line: &str) {
    out.write_all(line.as_bytes()).expect("write");
    out.write_all(b"\n").expect("write");
}

/// A full success `response` line echoing the command's `id` (REQUIRED for
/// correlation — a response without an `id` is dropped by the client).
fn response_line(id: &str, command: &str, data: &str) -> String {
    format!(
        r#"{{"type":"response","id":"{id}","command":"{command}","success":true,"data":{data}}}"#,
    )
}
