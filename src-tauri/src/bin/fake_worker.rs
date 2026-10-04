//! The `fake_worker` test fixture (ADR 0025 Task 3 — the
//! `fake_mcp_stdio` pattern): a tiny binary that speaks the Worker
//! protocol via the shared `protocol` types — `ready` on start, a
//! canned event + store-frame + `SinkFrame` stream on `start` +
//! `prompt`, a permission round-trip, `close` → exit 0, and a
//! `__crash__` prompt → exit 137.
//!
//! Wire behavior (one JSONL line per frame — the `protocol` types):
//! - startup: a `ready` line (the fixture version — BEFORE `start`).
//! - `start`: stores the `session_id` + `enabled_tools` (the advertised
//!   tool set rides a `session-update` `SinkFrame` — the
//!   `SubagentCapture` / wire-convention test input).
//! - `prompt`: a canned `RpcEvent` stream (`turn_start` →
//!   `message_start` / `message_end` with a canned assistant `Value` →
//!   `turn_end` → `agent_settled`) PLUS canned store frames
//!   (`TranscriptInsert` for a canned user + assistant `ChatMessage`,
//!   `DisplayUpsert` for a canned `agent-text` row) PLUS a canned
//!   `SinkFrame` `session-update` with an `agent_message_chunk` (the
//!   `SubagentCapture` test input — the enveloped shape).
//! - a `prompt` whose text is `"__crash__"`: exits 137 immediately
//!   (NO `agent_settled` — the crash-detection test).
//! - a `prompt` whose text is `"__permission__"`: a
//!   `PermissionRequest` (id `"p1"`, the FULL canned gate payload —
//!   `requestId` + `request: { toolCall: { title }, options }` —
//!   mirroring `permission.rs:135-158`); on a `PermissionResponse`
//!   (ANY `outcome`) → the tool-execution events + `agent_settled`
//!   (the round-trip test).
//! - a `prompt` whose text is `"__slow__"`: sleeps 10 s before
//!   settling (the `settle_timeout` tests).
//! - a `prompt` whose text is `"__subagent__"` /
//!   `"__slow_subagent__"` / `"__subagent_tools__"`: a canned
//!   `SubagentDispatch` frame (the Supervisor-side dispatch-flow test
//!   input — the `model_key` — `fake/m1` — resolves against the
//!   parent's `StartEnv` catalog; `__slow_subagent__` is the same
//!   frame with the SLOW child task; `__subagent_tools__` is the
//!   frame with `launch.tools: Some(["subagent"])` — the
//!   empties-out-after-the-guard-minus case).
//! - a `prompt` whose text is `"__cancel__"`: a `SubagentCancel` frame
//!   (the `SubagentCancel` routing test).
//! - a `subagent-result` line: a `subagent-result-ack` `SinkFrame`
//!   (the `SubagentResult` round-trip observation point — the
//!   `outcome` rides verbatim).
//! - `close`: exit 0 (the Worker self-exits — the Supervisor does NOT
//!   close the stdin pipe).

use std::io::{BufRead, Write};

use archimedes_lib::agent::events::RpcEvent;
use archimedes_lib::agent::harness::store::DisplayRow;
use archimedes_lib::agent::subagent::LaunchConfig;
use archimedes_lib::agent::worker::protocol::{decode_inbound, encode_outbound, Inbound, Outbound};

/// The fixture version (the `ready` frame's `version` field).
const VERSION: &str = "fake-worker-0.0.0";

/// Emit one outbound frame (one JSONL line — the `protocol` encode).
fn emit<W: Write>(stdout: &mut W, frame: &Outbound) {
    let line = encode_outbound(frame);
    let _ = stdout.write_all(line.as_bytes());
    let _ = stdout.flush();
}

/// The canned `RpcEvent` stream (the Supervisor's bookkeeping input —
/// `turn_start` → `message_start` / `message_end` with a canned
/// assistant `Value` → `turn_end` → `agent_settled`) PLUS the canned
/// store frames (`TranscriptInsert` for a canned user + assistant
/// `ChatMessage`, `DisplayUpsert` for a canned `agent-text` row) PLUS
/// a canned `SinkFrame` `session-update` with an `agent_message_chunk`
/// (the `SubagentCapture` test input — the enveloped shape).
///
/// The emission order mirrors the REAL harness (ADR 0025): the
/// `SinkFrame` `agent_message_chunk`s stream DURING the message
/// (BEFORE `agent_settled` — the `SubagentCapture` is fully fed
/// before the settle signal), and the store frames (the transcript
/// persistence) land with the message completion — ALSO before
/// `agent_settled`.
fn emit_canned_turn<W: Write>(stdout: &mut W, session_id: &str) {
    // The `RpcEvent` stream (the Supervisor's INTERNAL bookkeeping —
    // settle detection, the `SubagentCapture`'s usage, the debug
    // log).
    emit(
        stdout,
        &Outbound::Event {
            event: RpcEvent::turn_start,
        },
    );
    emit(
        stdout,
        &Outbound::Event {
            event: RpcEvent::message_start {
                message: serde_json::json!({ "role": "assistant" }),
            },
        },
    );
    // The `message_update` usage (the `SubagentCapture`'s token fields
    // — the metrics' token fields).
    emit(
        stdout,
        &Outbound::Event {
            event: RpcEvent::message_update {
                usage: serde_json::json!({ "inputTokens": 10, "outputTokens": 5 }),
                assistant_message_event: serde_json::json!({
                    "type": "text_delta",
                    "contentIndex": 0,
                    "delta": "canned "
                }),
            },
        },
    );
    // The `SinkFrame` `session-update` with the `agent_message_chunk`
    // stream (the `SubagentCapture` test input — the enveloped
    // `{ sessionId, update }` shape — streamed DURING the message, BEFORE
    // `agent_settled`).
    emit(
        stdout,
        &Outbound::SinkFrame {
            event: "session-update".to_string(),
            payload: serde_json::json!({
                "sessionId": session_id,
                "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "messageId": "m1",
                    "content": { "type": "text", "text": "canned " }
                }
            }),
        },
    );
    emit(
        stdout,
        &Outbound::SinkFrame {
            event: "session-update".to_string(),
            payload: serde_json::json!({
                "sessionId": session_id,
                "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "messageId": "m1",
                    "content": { "type": "text", "text": "answer" }
                }
            }),
        },
    );
    // The `message_end` with a canned assistant `Value` (the
    // `message_end` shape — carries NO text for tool-call messages,
    // so the `SubagentCapture` does NOT read it).
    emit(
        stdout,
        &Outbound::Event {
            event: RpcEvent::message_end {
                message: serde_json::json!({
                    "role": "assistant",
                    "content": [{ "type": "text", "text": "canned answer" }]
                }),
            },
        },
    );
    // The store frames (the Supervisor's persister input — the
    // `IpcStore`'s `TranscriptInsert` / `DisplayUpsert` frames — the
    // transcript rows land with the message completion, BEFORE
    // `agent_settled`).
    emit(
        stdout,
        &Outbound::TranscriptInsert {
            session_id: session_id.to_string(),
            seq: 1,
            role: "user".to_string(),
            content_json: r#"{"role":"user","content":"canned prompt"}"#.to_string(),
        },
    );
    emit(
        stdout,
        &Outbound::TranscriptInsert {
            session_id: session_id.to_string(),
            seq: 2,
            role: "assistant".to_string(),
            content_json: r#"{"role":"assistant","content":"canned answer"}"#.to_string(),
        },
    );
    emit(
        stdout,
        &Outbound::DisplayUpsert {
            session_id: session_id.to_string(),
            rows: vec![DisplayRow {
                kind: "agent-text".to_string(),
                message_key: "m1".to_string(),
                payload_json: serde_json::json!({ "text": "canned answer" }),
                created_at: 1,
            }],
        },
    );
    emit(
        stdout,
        &Outbound::Event {
            event: RpcEvent::turn_end {
                message: serde_json::json!({ "role": "assistant" }),
                tool_results: Vec::new(),
            },
        },
    );
    // The settle (LAST — the `SubagentCapture` is fully fed by now:
    // the `agent_message_chunk` stream + the usage are all delivered
    // before this frame, so the drive task's settle→teardown reads a
    // complete capture — no race).
    emit(
        stdout,
        &Outbound::Event {
            event: RpcEvent::agent_settled,
        },
    );
}

/// The tool-execution events + `agent_settled` (the
/// `__permission__` round-trip's post-response stream — the
/// tool-execution events arrive AFTER the `PermissionResponse`).
fn emit_tool_execution_and_settle<W: Write>(stdout: &mut W, session_id: &str) {
    let _ = session_id;
    emit(
        stdout,
        &Outbound::Event {
            event: RpcEvent::tool_execution_start {
                tool_call_id: "t1".to_string(),
                tool_name: "bash".to_string(),
                args: serde_json::json!({ "command": "ls" }),
            },
        },
    );
    emit(
        stdout,
        &Outbound::Event {
            event: RpcEvent::tool_execution_end {
                tool_call_id: "t1".to_string(),
                tool_name: "bash".to_string(),
                result: serde_json::json!({
                    "content": [{ "type": "text", "text": "ok" }],
                    "details": { "exitCode": 0 },
                    "isError": false
                }),
                is_error: false,
            },
        },
    );
    emit(
        stdout,
        &Outbound::Event {
            event: RpcEvent::agent_settled,
        },
    );
}

/// The canned `SubagentDispatch` frame (the Supervisor-side
/// dispatch-flow test input — the `model_key` — `fake/m1` — resolves
/// against the parent's `StartEnv` catalog; the `task` is the
/// subagent's initial prompt — `"__slow__"` for the `settle_timeout`
/// tests, `"__subagent_tools__"`'s `launch.tools` is
/// `Some(["subagent"])` — the empties-out-after-the-guard-minus
/// case).
fn emit_subagent_dispatch<W: Write>(
    stdout: &mut W,
    session_id: &str,
    task: &str,
    tools: Option<Vec<String>>,
    model_key: &str,
) {
    let parent_enabled_tools = if tools.is_some() {
        vec!["subagent".to_string()]
    } else {
        vec!["bash".to_string(), "read".to_string()]
    };
    emit(
        stdout,
        &Outbound::SubagentDispatch {
            id: "sub1".to_string(),
            parent_session_id: session_id.to_string(),
            parent_cwd: "/tmp/space".to_string(),
            parent_enabled_tools,
            agent_name: "tester".to_string(),
            launch: LaunchConfig {
                system_prompt: Some("be brief".to_string()),
                model: None,
                thinking: None,
                frontmatter_thinking: None,
                tools,
            },
            task: task.to_string(),
            model_key: model_key.to_string(),
        },
    );
}

fn main() {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    // The `ready` frame (the fixture version — emitted at startup,
    // BEFORE `start`).
    emit(
        &mut stdout,
        &Outbound::Ready {
            version: VERSION.to_string(),
            session_id: None,
        },
    );

    let mut reader = stdin.lock();
    let mut line = String::new();
    let mut session_id: Option<String> = None;
    // The parent's `model` key (the `Start` env — the `SubagentDispatch`
    // frame's `model_key` — the Supervisor resolves it against the
    // parent's `StartEnv` catalog; the fixture's hardcoded `fake/m1`
    // would be unknown when the parent's model is a different provider
    // (e.g. the `ipc` test's seeded `tama/m1`)).
    let mut model_key: Option<String> = None;

    while let Ok(n) = reader.read_line(&mut line) {
        if n == 0 {
            break; // EOF — the Supervisor is gone.
        }
        let l = line.trim_end_matches(['\n', '\r']).to_string();
        line.clear();
        if l.is_empty() {
            continue;
        }
        let Ok(msg) = decode_inbound(&l) else {
            continue; // a malformed line — the fixture is permissive.
        };
        let sid = session_id.clone().unwrap_or_default();
        match msg {
            Inbound::Start(env) => {
                session_id = Some(env.session_id.clone());
                model_key = Some(format!("{}/{}", env.model.provider, env.model.id));
                // The advertised tool set (the `SubagentCapture` /
                // wire-convention test input — a `session-update`
                // `SinkFrame` with the `enabledTools` field — the
                // harness convention verbatim: `None` = all,
                // `Some(v)` = exactly `v`, `Some([])` = NO tools).
                // The `systemPrompt` + `trusted` fields ride the envelope
                // (the ADR 0025 Task 5 test input — the subagent's
                // `StartEnv` envelope assertion: the `systemPrompt` is
                // the `build_child_system_message` output, the `trusted`
                // is the parent's trust flag (the inheritance — ADR
                // 0010)). `systemPrompt` is ALWAYS present (a `null`
                // when the env's field is `None` — the absence-of-a-
                // system-message assertion keys on the `null`).
                let update = serde_json::json!({
                    "sessionUpdate": "session_info",
                    "enabledTools": env.enabled_tools,
                    "systemPrompt": serde_json::to_value(&env.system_prompt).unwrap(),
                    "trusted": env.trusted
                });
                emit(
                    &mut stdout,
                    &Outbound::SinkFrame {
                        event: "session-update".to_string(),
                        payload: serde_json::json!({
                            "sessionId": env.session_id,
                            "update": update
                        }),
                    },
                );
            }
            Inbound::Prompt { text, .. } => match text.as_str() {
                // The crash (NO `agent_settled` — the
                // crash-detection test; the process exits 137 —
                // the SIGKILL convention).
                "__crash__" => std::process::exit(137),
                // The permission round-trip: the `PermissionRequest`
                // (id `"p1"`, the FULL canned gate payload —
                // `requestId` + `request: { toolCall: { title },
                // options }` — mirroring `permission.rs:135-158`);
                // on a `PermissionResponse` (ANY `outcome`) → the
                // tool-execution events + `agent_settled`.
                "__permission__" => {
                    emit(
                        &mut stdout,
                        &Outbound::PermissionRequest {
                            id: "p1".to_string(),
                            payload: serde_json::json!({
                                "sessionId": sid,
                                "requestId": "p1",
                                "request": {
                                    "sessionId": sid,
                                    "toolCall": { "title": "Allow bash?" },
                                    "options": [
                                        { "optionId": "allow", "name": "Allow", "kind": "allow" },
                                        { "optionId": "reject", "name": "Block", "kind": "reject" },
                                        { "optionId": "trust-space", "name": "Don't ask again for this Space", "kind": "allow" }
                                    ]
                                }
                            }),
                        },
                    );
                    // Wait for the `PermissionResponse` (ANY
                    // `outcome` — the fixture is permissive; a
                    // different frame while waiting is ignored).
                    loop {
                        let Ok(n) = reader.read_line(&mut line) else {
                            std::process::exit(1); // the Supervisor is gone.
                        };
                        if n == 0 {
                            std::process::exit(1); // EOF — the Supervisor is gone.
                        }
                        let l2 = line.trim_end_matches(['\n', '\r']).to_string();
                        line.clear();
                        if l2.is_empty() {
                            continue;
                        }
                        let Ok(m2) = decode_inbound(&l2) else {
                            continue;
                        };
                        if matches!(m2, Inbound::PermissionResponse { .. }) {
                            break;
                        }
                    }
                    // The tool-execution events + `agent_settled`
                    // (the round-trip completes — the
                    // `PermissionOutcome` was delivered verbatim).
                    emit_tool_execution_and_settle(&mut stdout, &sid);
                }
                // The slow settle (the `settle_timeout` tests — a 10 s
                // sleep before settling; the `settle_timeout` bound is
                // shorter, so the timeout wins).
                "__slow__" => {
                    std::thread::sleep(std::time::Duration::from_secs(10));
                    emit_canned_turn(&mut stdout, &sid);
                }
                // The `interactive-event` `SinkFrame` (the `todos_update`
                // push — the frontend's `useInteractive.applyTodoUpdate`
                // consumes the payload VERBATIM) + `agent_settled` (the
                // `headless` IPC test's `__interactive__` input — the
                // `SinkFrame` re-emit contract observation point).
                "__interactive__" => {
                    emit(
                        &mut stdout,
                        &Outbound::SinkFrame {
                            event: "interactive-event".to_string(),
                            payload: serde_json::json!({
                                "sessionId": sid,
                                "seq": 0,
                                "event": "todos_update",
                                "payload": {
                                    "source": "main",
                                    "todos": [
                                        { "content": "do the thing", "status": "pending" }
                                    ]
                                }
                            }),
                        },
                    );
                    emit(
                        &mut stdout,
                        &Outbound::Event {
                            event: RpcEvent::agent_settled,
                        },
                    );
                }
                // The Supervisor-side dispatch-flow test input (a
                // canned `SubagentDispatch` frame — the `model_key`
                // — `fake/m1` — resolves against the parent's
                // `StartEnv` catalog).
                "__subagent__" => {
                    emit_subagent_dispatch(
                        &mut stdout,
                        &sid,
                        "the task",
                        None,
                        &model_key.clone().unwrap_or_else(|| "fake/m1".to_string()),
                    );
                    // The parent's turn settles AFTER the dispatch (the
                    // fixture's simplified model — the `send_prompt`
                    // helper waits for the settle; the `SubagentResult`
                    // arrives later and is acked on receipt).
                    emit(
                        &mut stdout,
                        &Outbound::Event {
                            event: RpcEvent::agent_settled,
                        },
                    );
                }
                // The `settle_timeout` test input (the same frame with
                // the SLOW child task — the subagent's initial
                // prompt is `"__slow__"` → the 10 s settle).
                "__slow_subagent__" => {
                    emit_subagent_dispatch(
                        &mut stdout,
                        &sid,
                        "__slow__",
                        None,
                        &model_key.clone().unwrap_or_else(|| "fake/m1".to_string()),
                    );
                    emit(
                        &mut stdout,
                        &Outbound::Event {
                            event: RpcEvent::agent_settled,
                        },
                    );
                }
                // The wire-convention test input (the frame with
                // `launch.tools: Some(["subagent"])` — the
                // empties-out-after-the-guard-minus case; the
                // `parent_enabled_tools` is the `Some` case's parent
                // list).
                "__subagent_tools__" => {
                    emit_subagent_dispatch(
                        &mut stdout,
                        &sid,
                        "the task",
                        Some(vec!["subagent".to_string()]),
                        &model_key.clone().unwrap_or_else(|| "fake/m1".to_string()),
                    );
                    emit(
                        &mut stdout,
                        &Outbound::Event {
                            event: RpcEvent::agent_settled,
                        },
                    );
                }
                // The `SubagentCancel` routing test input (the loop's
                // `SubagentCancel` → the `IpcDispatcher`'s
                // `SubagentCancel` frame).
                "__cancel__" => {
                    emit(
                        &mut stdout,
                        &Outbound::SubagentCancel {
                            id: "sub1".to_string(),
                        },
                    );
                }
                // The default prompt (the canned turn — the full
                // event + store-frame + `SinkFrame` stream).
                _ => {
                    emit_canned_turn(&mut stdout, &sid);
                }
            },
            // The `SubagentResult` (the Supervisor's delivery — the
            // `IpcDispatcher` waiter resolves; the fixture's
            // `subagent-result-ack` `SinkFrame` is the round-trip
            // observation point — the `outcome` rides verbatim).
            Inbound::SubagentResult { id, outcome } => {
                emit(
                    &mut stdout,
                    &Outbound::SinkFrame {
                        event: "subagent-result-ack".to_string(),
                        payload: serde_json::json!({
                            "id": id,
                            "outcome": serde_json::to_value(&outcome).unwrap_or_default()
                        }),
                    },
                );
            }
            // The `close` — the Worker self-exits 0 (the Supervisor
            // does NOT close the stdin pipe on `close`).
            Inbound::Close => std::process::exit(0),
            // The `config` — echo the frame (a `config-ack` `SinkFrame`
            // — the `set_space_trusted` mid-session trust-toggle test's
            // delivery observation point: the `trusted` flag rides
            // verbatim; the `model` / `thinking` fields ride when set).
            Inbound::Config {
                model,
                thinking,
                trusted,
            } => {
                let mut payload = serde_json::json!({
                    "sessionId": sid,
                    "trusted": trusted
                });
                if let Some(m) = model {
                    payload["model"] = serde_json::to_value(&m).unwrap_or_default();
                }
                if let Some(t) = thinking {
                    payload["thinking"] = serde_json::Value::String(t);
                }
                emit(
                    &mut stdout,
                    &Outbound::SinkFrame {
                        event: "config-ack".to_string(),
                        payload,
                    },
                );
            }
            // `abort` / a `permission-response` to an unknown id / an
            // `interactive-response` / an unknown `type`: no response
            // (the fixture is permissive).
            _ => {}
        }
    }
    // stdin EOF — the Supervisor is gone (exit 0 — the clean-EOF
    // path).
}
