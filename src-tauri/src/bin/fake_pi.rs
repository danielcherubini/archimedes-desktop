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
/// The `get_state` payload template: `__SESSION_ID__` is replaced with the
/// process's session id (a fresh process = a unique `fake-pi-<hex>` id,
/// mirroring real pi; a `--session <file>` load = the file's stem — the
/// loaded session's own id, the way real pi reads it from the file).
const GET_STATE_DATA: &str = r#"{"sessionId":"__SESSION_ID__","sessionFile":"/tmp/fake-pi-session.jsonl","model":{"provider":"__PROVIDER__","id":"__MODEL_ID__","name":"Fake Model","contextWindow":100000,"cost":{}},"thinkingLevel":"__LEVEL__","isStreaming":false,"steeringMode":"all","followUpMode":"all","autoCompactionEnabled":true,"messageCount":0,"pendingMessageCount":0}"#;

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

/// The `FAKE_PI_TWO_MSGS` turn sequence: a USER `message_start`, then TWO
/// assistant messages (m1 = "hello", m2 = "world" — each a single
/// `text_delta` + `message_end`), then `agent_end` + `agent_settled`.
/// (The subagent text-capture test needs two distinct `messageId`s.)
const ASSISTANT_MESSAGE_START_2: &str = r#"{"type":"message_start","message":{"role":"assistant","content":[],"api":"fake","provider":"fake","model":"fake-model","usage":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}},"timestamp":"2026-09-24T00:00:05.000Z"}}"#;
const TEXT_DELTA_M1: &str = r#"{"type":"message_update","usage":__USAGE__,"assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"hello"}}"#;
const MESSAGE_END_M1: &str = r#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"hello"}],"api":"fake","provider":"fake","model":"fake-model","usage":{"input":1,"output":1,"cacheRead":0,"cacheWrite":0,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}},"stopReason":"stop","timestamp":"2026-09-24T00:00:06.000Z"}}"#;
const TEXT_DELTA_M2: &str = r#"{"type":"message_update","usage":__USAGE__,"assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"world"}}"#;
const MESSAGE_END_M2: &str = r#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"world"}],"api":"fake","provider":"fake","model":"fake-model","usage":{"input":1,"output":1,"cacheRead":0,"cacheWrite":0,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}},"stopReason":"stop","timestamp":"2026-09-24T00:00:07.000Z"}}"#;

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

/// The process's session id: `--session <file>` → the file's STEM (the
/// loaded session's own id — the way real pi reads it from the session
/// file); otherwise a UNIQUE `fake-pi-<hex>` id (mirroring real pi's
/// per-process session ids — two live sessions of the same agent have
/// distinct ids).
fn session_id_from_args() -> String {
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--session" {
            if let Some(p) = args.next() {
                let stem = std::path::Path::new(&p)
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| p.clone());
                return stem;
            }
        }
    }
    // A fresh session: a unique id (the `std::process::id` + a counter is
    // not stable across re-forks; a random hex from the time + pid is
    // unique enough for tests).
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("fake-pi-{:x}{:x}", std::process::id(), nanos)
}

fn main() {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());

    // The session's state (the `set_model` / `set_thinking_level` commands
    // update it; `get_state` reflects it — the way the real agent does).
    let mut current_provider = "fake".to_string();
    let mut current_model_id = "fake-model".to_string();
    let mut current_level = "off".to_string();

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
                // Record the prompt payload (the `FAKE_PI_ECHO_PROMPT` /
                // dispatch subagent modes read the task from it).
                record_prompt(&v);
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
                } else if is_set("FAKE_PI_DISPATCH")
                    || is_set("FAKE_PI_DISPATCH_TWO")
                    || is_set("FAKE_PI_DISPATCH_CANCEL")
                {
                    // The subagent-dispatch E2E (Task 5): the MAIN (no
                    // `ARCHIMEDES_SUBAGENT`) fires the `dispatch_subagent`
                    // bridge frame(s) and settles with the echoed
                    // response(s); the SUBAGENT (`ARCHIMEDES_SUBAGENT=1`)
                    // echoes its task (or hangs, in the cancel mode).
                    handle_dispatch(&mut out);
                } else if is_set("FAKE_PI_ECHO_PROMPT") {
                    // The assistant message's text = the prompt text
                    // verbatim (the subagent `echo` variant).
                    emit_echo_turn(&mut out);
                } else if is_set("FAKE_PI_NO_TEXT") {
                    // A turn with NO assistant message (the no-text
                    // subagent — the stale-carry-over test's input).
                    emit_no_text_turn(&mut out);
                } else if is_set("FAKE_PI_HANG_PROMPT") {
                    // The preflight response only (NO turn events): the
                    // prompt never settles (the cancel E2E's subagent).
                } else if is_set("FAKE_PI_TWO_MSGS") {
                    // Two assistant messages (m1 "hello" + m2 "world").
                    emit_two_msgs(&mut out);
                } else {
                    if is_set("FAKE_PI_UNKNOWN") {
                        // An unknown event type before settling (permissive
                        // deserialization check).
                        write_line(&mut out, UNKNOWN_EVENT);
                    }
                    if is_set("FAKE_PI_WAIT_ABORT") {
                        // The turn's events WITHOUT the settle: the turn
                        // stays in-flight until an `abort` arrives (which
                        // settles it — the cancel test's input).
                        emit_turn_no_settle(&mut out);
                    } else {
                        // The turn's event sequence (the fake "LLM" output).
                        emit_turn(&mut out);
                    }
                }
            }
            "get_state" => {
                let data = GET_STATE_DATA
                    .replace("__SESSION_ID__", &session_id_from_args())
                    .replace("__PROVIDER__", &current_provider)
                    .replace("__MODEL_ID__", &current_model_id)
                    .replace("__LEVEL__", &current_level);
                write_line(&mut out, &response_line(id, "get_state", &data));
            }
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
            // The REQUESTED provider/modelId (the new current model —
            // tracked so the next `get_state` reflects it, the way the
            // real agent does).
            "set_model" => {
                let data = v
                    .get("provider")
                    .and_then(|p| p.as_str())
                    .zip(
                        v.get("modelId")
                            .and_then(|m| m.as_str()),
                    )
                    .map(|(provider, model_id)| {
                        current_provider = provider.to_string();
                        current_model_id = model_id.to_string();
                        format!(
                            r#"{{"type":"response","id":"{id}","command":"set_model","success":true,"data":{{"provider":"{provider}","id":"{model_id}","name":"Fake Model","contextWindow":100000,"cost":{{}}}}}}"#
                        )
                    })
                    .unwrap_or_else(|| {
                        format!(r#"{{"type":"response","id":"{id}","command":"set_model","success":false,"error":"missing provider/modelId"}}"#)
                    });
                write_line(&mut out, &data);
            }
            "set_thinking_level" => {
                if let Some(level) = v.get("level").and_then(|l| l.as_str()) {
                    current_level = level.to_string();
                }
                write_line(
                    &mut out,
                    &format!(
                        r#"{{"type":"response","id":"{id}","command":"set_thinking_level","success":true}}"#
                    ),
                );
            }
            "abort" => {
                // The real agent responds to `abort` (rpc-mode.js:329-331:
                // `await session.abort(); return success(id, "abort")`) and the
                // aborted turn settles — emit the response FIRST (so the
                // client's `send(abort)` resolves), then the settle event.
                write_line(
                    &mut out,
                    &format!(
                        r#"{{"type":"response","id":"{id}","command":"abort","success":true}}"#,
                    ),
                );
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

/// The turn's event sequence WITHOUT the `agent_settled` (the
/// `FAKE_PI_WAIT_ABORT` mode — the turn stays in-flight until an `abort`
/// arrives, which settles it).
fn emit_turn_no_settle(out: &mut std::io::BufWriter<std::io::StdoutLock>) {
    write_line(out, USER_MESSAGE_START);
    write_line(out, ASSISTANT_MESSAGE_START);
    write_line(out, &TEXT_DELTA_1.replace("__USAGE__", USAGE));
    write_line(out, &TEXT_DELTA_2.replace("__USAGE__", USAGE));
    write_line(out, MESSAGE_END);
    write_line(out, AGENT_END);
    out.flush().expect("flush");
}

/// The prompt text (`message` or `content` — the desktop sends `message`
/// for main prompts and `content` for subagent tasks).
fn prompt_text(v: &serde_json::Value) -> String {
    v.get("message")
        .or_else(|| v.get("content"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A turn whose assistant text is `text` (the `FAKE_PI_ECHO_PROMPT` /
/// dispatch-echo turns).
fn emit_text_turn(out: &mut std::io::BufWriter<std::io::StdoutLock>, text: &str) {
    write_line(out, USER_MESSAGE_START);
    write_line(
        out,
        &ASSISTANT_MESSAGE_START
            .replace("__PROVIDER__", "fake")
            .replace("__MODEL_ID__", "fake-model"),
    );
    write_line(
        out,
        &TEXT_DELTA_1
            .replace("__USAGE__", USAGE)
            .replace("Hel", text),
    );
    write_line(out, &MESSAGE_END.replace("Hello", text));
    write_line(out, AGENT_END);
    write_line(out, AGENT_SETTLED);
    out.flush().expect("flush");
}

/// The `FAKE_PI_ECHO_PROMPT` turn: the assistant text = the prompt text
/// verbatim.
fn emit_echo_turn(out: &mut std::io::BufWriter<std::io::StdoutLock>) {
    let text = prompt_text(&last_prompt());
    emit_text_turn(out, &text);
}

/// The `FAKE_PI_NO_TEXT` turn: NO assistant message (the no-text
/// subagent — the stale-carry-over test's input).
fn emit_no_text_turn(out: &mut std::io::BufWriter<std::io::StdoutLock>) {
    write_line(out, USER_MESSAGE_START);
    write_line(out, AGENT_END);
    write_line(out, AGENT_SETTLED);
    out.flush().expect("flush");
}

/// The `dispatch_subagent` bridge frame (the shape the desktop's
/// `dispatch_params` parses).
fn dispatch_frame(id: &str, task: &str) -> String {
    serde_json::json!({
        "v": 1,
        "type": "request",
        "id": id,
        "method": "dispatch_subagent",
        "source": "main",
        "params": {
            "agentName": "fake",
            "task": task,
            "systemPrompt": null,
            "model": null,
            "thinking": null,
            "tools": null
        }
    })
    .to_string()
}

/// The bridge stream (a `Read` + `Write` supertrait — a trait object cannot
/// name two non-auto traits; the blanket impl keeps it usable).
trait BridgeStream: std::io::Read + std::io::Write + Send {}
impl<T: std::io::Read + std::io::Write + Send> BridgeStream for T {}
type BridgeConn = Box<dyn BridgeStream>;

/// Connect the agent's OWN bridge socket (the desktop's listener) and
/// return the connected stream (the `dispatch-two` concurrent mode reads
/// from two connections).
fn connect_bridge_socket(socket: &str) -> std::io::Result<BridgeConn> {
    #[cfg(unix)]
    {
        Ok(Box::new(std::os::unix::net::UnixStream::connect(socket)?))
    }
    #[cfg(not(unix))]
    {
        let _ = socket;
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "bridge unavailable on this platform",
        ))
    }
}

/// Read the `dispatch_subagent` response line and extract the RAW result
/// (`<output>` / `<error>` / `unknown` / `cancelled` — the caller prefixes
/// the `dispatch` marker).
fn read_dispatch_result(stream: &mut BridgeConn) -> String {
    let mut reader = std::io::BufReader::new(stream);
    let mut line = String::new();
    match reader.read_line(&mut line) {
        Ok(0) => "cancelled".to_string(),
        Ok(_) => {
            let resp: serde_json::Value =
                serde_json::from_str(line.trim()).unwrap_or(serde_json::Value::Null);
            if let Some(output) = resp
                .get("result")
                .and_then(|r| r.get("output"))
                .and_then(serde_json::Value::as_str)
            {
                output.to_string()
            } else if let Some(err) = resp.get("error").and_then(serde_json::Value::as_str) {
                err.to_string()
            } else {
                "unknown".to_string()
            }
        }
        Err(_) => "read-error".to_string(),
    }
}

/// The `FAKE_PI_DISPATCH*` modes (Task 5's subagent-dispatch E2E). The
/// `ARCHIMEDES_SUBAGENT=1` env (set by the desktop on subagent spawns)
/// selects the subagent behavior; otherwise the MAIN behavior.
fn handle_dispatch(out: &mut std::io::BufWriter<std::io::StdoutLock>) {
    let socket = std::env::var("PI_ARCHIMEDES_BRIDGE_SOCKET").unwrap_or_default();
    let is_subagent = std::env::var("ARCHIMEDES_SUBAGENT").is_ok();

    if is_subagent {
        if is_set("FAKE_PI_COST_PUSH") {
            // Push two `cost_update` frames through the bridge (one
            // connection per push, the `ack` read BEFORE the next — the
            // desktop's end-of-turn capture is complete) + the default
            // turn.
            emit_cost_push_turn(out);
            return;
        }
        if is_set("FAKE_PI_DISPATCH_CANCEL") {
            // The subagent HANGS on its prompt (no turn events — the
            // desktop's cancel tears it down; the parent close is the
            // cancel trigger).
            return;
        }
        // The subagent echoes its task (the `echo` variant); the
        // `EMPTY` sentinel is a NO-TEXT turn (the stale-carry-over test).
        let task = prompt_text(&last_prompt());
        if task == "EMPTY" {
            emit_no_text_turn(out);
        } else {
            emit_text_turn(out, &task);
        }
        return;
    }

    // The MAIN: fire the `dispatch_subagent` frame(s).
    let mut stream = match connect_bridge_socket(&socket) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("fake_pi: dispatch connect failed: {e}");
            emit_text_turn(out, "dispatch:connect-error");
            return;
        }
    };

    if is_set("FAKE_PI_DISPATCH_CANCEL") {
        // Write the frame, hold the connection briefly (a real aborting
        // main agent was WAITING for the response — its connection stays
        // open while it waits; the delay lets the subagent ESTABLISH
        // before the abort cancels it), then close WITHOUT reading (the
        // parent close cancels the in-flight dispatch).
        let task =
            std::env::var("FAKE_PI_DISPATCH_TASK").unwrap_or_else(|_| "do the task".to_string());
        let frame = dispatch_frame("fake-dispatch-cancel", &task);
        let data = frame + "\n";
        let _ = std::io::Write::write_all(&mut *stream, data.as_bytes());
        let _ = std::io::Write::flush(&mut *stream);
        std::thread::sleep(std::time::Duration::from_millis(300));
        // Drop the stream (close the connection) + settle with a marker.
        drop(stream);
        emit_text_turn(out, "dispatch:aborted");
        return;
    }

    let tasks: Vec<String> = if is_set("FAKE_PI_DISPATCH_TWO") {
        vec![
            std::env::var("FAKE_PI_DISPATCH_TASK").unwrap_or_else(|_| "task-one".to_string()),
            std::env::var("FAKE_PI_DISPATCH_TASK_2").unwrap_or_else(|_| "task-two".to_string()),
        ]
    } else {
        vec![std::env::var("FAKE_PI_DISPATCH_TASK").unwrap_or_else(|_| "do the task".to_string())]
    };

    let sequential = is_set("FAKE_PI_DISPATCH_TWO_SEQUENTIAL");
    let echoes: Vec<String> = if tasks.len() == 1 {
        // One frame, one response (the `dispatch:` prefix — the single
        // dispatch's echo marker).
        let mut s = stream;
        let frame = dispatch_frame("fake-dispatch-1", &tasks[0]);
        let data = frame + "\n";
        let _ = std::io::Write::write_all(&mut s, data.as_bytes());
        let _ = std::io::Write::flush(&mut s);
        vec![format!("dispatch:{}", read_dispatch_result(&mut s))]
    } else if sequential {
        // SEQUENTIAL: frame 1 → read response 1 → frame 2 → read response 2.
        // ONE connection per frame (the desktop's bridge protocol is one
        // connection per message — reusing a connection for a second frame
        // would be drained and discarded after the first response).
        let mut echoes = Vec::new();
        for (i, t) in tasks.iter().enumerate() {
            let s = match connect_bridge_socket(&socket) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("fake_pi: dispatch-two connect failed: {e}");
                    emit_text_turn(out, "dispatch:connect-error");
                    return;
                }
            };
            let mut s = s;
            let frame = dispatch_frame(&format!("fake-dispatch-{}", i + 1), t);
            let data = frame + "\n";
            let _ = std::io::Write::write_all(&mut s, data.as_bytes());
            let _ = std::io::Write::flush(&mut s);
            // The sequential prefix is added at the settle (below).
            echoes.push(read_dispatch_result(&mut s));
        }
        echoes
    } else {
        // CONCURRENT: write BOTH frames (on two connections) BEFORE reading
        // either response (the desktop dispatches the two subagents
        // concurrently on the worker runtime).
        let mut s1 = stream;
        let mut s2 = match connect_bridge_socket(&socket) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("fake_pi: dispatch-two connect failed: {e}");
                emit_text_turn(out, "dispatch:connect-error");
                return;
            }
        };
        let d1 = dispatch_frame("fake-dispatch-1", &tasks[0]) + "\n";
        let d2 = dispatch_frame("fake-dispatch-2", &tasks[1]) + "\n";
        let _ = std::io::Write::write_all(&mut *s1, d1.as_bytes());
        let _ = std::io::Write::flush(&mut *s1);
        let _ = std::io::Write::write_all(&mut *s2, d2.as_bytes());
        let _ = std::io::Write::flush(&mut *s2);
        let mut r1 = s1;
        let mut r2 = s2;
        vec![read_dispatch_result(&mut r1), read_dispatch_result(&mut r2)]
    };

    // Settle with the echo(es): `dispatch:<r>` (one) or `dispatch1:<r1>` +
    // `dispatch2:<r2>` (two text deltas — the `dispatchN:` prefix is the
    // per-dispatch marker; the raw result is the subagent's own output).
    if echoes.len() == 1 {
        emit_text_turn(out, &echoes[0]);
    } else {
        let d1 = format!("dispatch1:{}", echoes[0]);
        let d2 = format!("dispatch2:{}", echoes[1]);
        // JSON-escape the delta values (the task text may contain quotes or
        // backslashes — a raw splice would corrupt the JSONL line, and the
        // reader treats a malformed line as a FATAL `Parse` error).
        let d1_json = serde_json::to_string(&d1).expect("valid json");
        let d2_json = serde_json::to_string(&d2).expect("valid json");
        write_line(out, USER_MESSAGE_START);
        write_line(out, ASSISTANT_MESSAGE_START);
        write_line(
            out,
            &TEXT_DELTA_1
                .replace("__USAGE__", USAGE)
                .replace("\"Hel\"", &d1_json),
        );
        write_line(
            out,
            &TEXT_DELTA_2
                .replace("__USAGE__", USAGE)
                .replace("\"lo\"", &d2_json),
        );
        // The `message_end` carries the full message text (the two deltas
        // concatenated), matching the real pi's turn shape.
        let text_json = serde_json::to_string(&format!("{d1}{d2}")).expect("valid json");
        let message_end = format!(
            "{{\"type\":\"message_end\",\"message\":{{\"role\":\"assistant\",\"content\":[{{\"type\":\"text\",\"text\":{text_json}}}],\"api\":\"fake\",\"provider\":\"fake\",\"model\":\"fake-model\",\"usage\":{USAGE},\"stopReason\":\"stop\",\"timestamp\":\"2026-09-24T00:00:04.000Z\"}}}}"
        );
        write_line(out, &message_end);
        write_line(out, AGENT_END);
        write_line(out, AGENT_SETTLED);
        out.flush().expect("flush");
    }
}

/// Push two `cost_update` frames through the agent's OWN bridge socket
/// (the `FAKE_PI_COST_PUSH` mode — the metrics-capture E2E), then emit the
/// default turn. A missing bridge socket degrades to the default turn (the
/// capture is simply empty).
fn emit_cost_push_turn(out: &mut std::io::BufWriter<std::io::StdoutLock>) {
    let socket = match std::env::var("PI_ARCHIMEDES_BRIDGE_SOCKET") {
        Ok(s) => s,
        Err(_) => {
            emit_turn(out);
            return;
        }
    };
    let payloads = [
        serde_json::json!({ "inputTokens": 100, "outputTokens": 50, "cost": 0.001 }),
        serde_json::json!({ "inputTokens": 200, "outputTokens": 25, "cacheReadTokens": 10, "cost": 0.002 }),
    ];
    for (i, payload) in payloads.iter().enumerate() {
        let frame = serde_json::json!({
            "v": 1,
            "type": "push",
            "seq": i + 1,
            "event": "cost_update",
            "payload": payload,
        });
        let mut stream = match connect_bridge_socket(&socket) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("fake_pi: cost-push connect failed: {e}");
                break;
            }
        };
        let data = frame.to_string() + "\n";
        if std::io::Write::write_all(&mut stream, data.as_bytes()).is_err() {
            break;
        }
        if std::io::Write::flush(&mut stream).is_err() {
            break;
        }
        // Read the `ack` line BEFORE the next push (the desktop's channel
        // protocol: one connection per message, acked before the next).
        let mut line = String::new();
        let mut reader = std::io::BufReader::new(&mut stream);
        if reader.read_line(&mut line).is_err() || line.trim() != "ack" {
            eprintln!("fake_pi: cost-push ack missing: {line:?}");
            break;
        }
    }
    emit_turn(out);
}

/// The last `prompt` command's payload (the `FAKE_PI_ECHO_PROMPT` /
/// dispatch subagent modes read the task from it).
static LAST_PROMPT: std::sync::OnceLock<std::sync::Mutex<serde_json::Value>> =
    std::sync::OnceLock::new();

fn record_prompt(v: &serde_json::Value) {
    LAST_PROMPT.get_or_init(|| std::sync::Mutex::new(serde_json::Value::Null));
    if let Some(slot) = LAST_PROMPT.get() {
        if let Ok(mut p) = slot.lock() {
            *p = v.clone();
        }
    }
}

fn last_prompt() -> serde_json::Value {
    LAST_PROMPT
        .get()
        .and_then(|s| s.lock().ok().map(|p| p.clone()))
        .unwrap_or(serde_json::Value::Null)
}

/// The `FAKE_PI_TWO_MSGS` turn sequence (two assistant messages, m1 then
/// m2 — the subagent text-capture test's input).
fn emit_two_msgs(out: &mut std::io::BufWriter<std::io::StdoutLock>) {
    write_line(out, USER_MESSAGE_START);
    write_line(out, ASSISTANT_MESSAGE_START);
    write_line(out, &TEXT_DELTA_M1.replace("__USAGE__", USAGE));
    write_line(out, MESSAGE_END_M1);
    write_line(out, ASSISTANT_MESSAGE_START_2);
    write_line(out, &TEXT_DELTA_M2.replace("__USAGE__", USAGE));
    write_line(out, MESSAGE_END_M2);
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
