//! A minimal fake ACP agent used by integration tests.
//!
//! Plain `std`, no async: reads newline-delimited JSON-RPC 2.0 frames from
//! stdin and writes them to stdout. It speaks just enough ACP to drive the
//! `acp_flow` / `subagent_dispatch` integration tests.
//!
//! The agent's behavior is selected by its first positional argument (the
//! "mode"):
//!
//! - *default* (no arg): `session/prompt` → two `session/update` notifications
//!   (`agent_message_chunk`, text `"hello"` then `" world"`, same
//!   `messageId` `"m1"`) then `stopReason: "end_turn"`.
//! - `two-msgs`: `session/prompt` → two `session/update` notifications with
//!   DISTINCT `messageId`s (`"m1"` → `"hello"`, then `"m2"` → `"world"`) then
//!   `stopReason: "end_turn"` (the `text_capture` / `last_message_id` unit
//!   test — the "last message" is `m2`, not derived from `HashMap` order).
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
//! - `set_config_option_error`: `session/set_config_option` is rejected.
//!
//! **Subagent modes (the `subagent-sessions` E2E, Task 6).** A single registry
//! entry is used for BOTH the main and subagent spawns (the desktop spawns the
//! subagent with the parent's registry entry). The `args: ["dispatch"]` apply
//! to both, so the mode is disambiguated by the **`PI_ACP_PI_COMMAND` env rule**:
//! when `PI_ACP_PI_COMMAND` is SET (the desktop sets it ONLY for subagent
//! spawns, via the per-dispatch launch wrapper), the agent behaves as the
//! SUBAGENT agent REGARDLESS of the positional arg; unset → the positional-arg
//! mode. The subagent variant is `FAKE_SUBAGENT_MODE` (`subagent` / `hang` /
//! `ask`), and the subagent reports a DISTINCT session id (`fake-subagent-1`,
//! overridable via `FAKE_SUBAGENT_SESSION_ID`) so the two sessions' events are
//! distinguishable:
//!
//! - `dispatch` (main): `session/prompt` → connect to `PI_ARCHIMEDES_BRIDGE_
//!   SOCKET`, write a `dispatch_subagent` request frame (the `task` from
//!   `FAKE_DISPATCH_TASK`), read the response line (blocking — the connection
//!   stays open until the first data, the desktop's contract), then echo
//!   `dispatch:<result.output>` (or `dispatch:<error>`) as a chunk + `end_turn`.
//! - `dispatch-two` (main, the shared-capture regression): send TWO
//!   `dispatch_subagent` frames (the tasks from `FAKE_DISPATCH_TASK` /
//!   `FAKE_DISPATCH_TASK_2`) and echo each response (`dispatch1:…`,
//!   `dispatch2:…`) + `end_turn`. CONCURRENT by default (both frames written
//!   BEFORE either response is read — the desktop dispatches the two subagents
//!   concurrently); `FAKE_DISPATCH_TWO_SEQUENTIAL=1` reads response 1 BEFORE
//!   sending frame 2 (a no-text dispatch AFTER a text one).
//! - `dispatch-cancel` (main): write the frame, hold the connection open
//!   briefly (the main was "waiting" for the response — a real aborting agent
//!   stays connected while it waits), then close it WITHOUT reading (the main
//!   agent's abort — the parent close cancels the in-flight dispatch), then
//!   echo a chunk + `end_turn` (the main stays live).
//! - `subagent` (default subagent variant): `session/prompt` → one chunk
//!   (`"subagent-done"`, `m1`) + `end_turn`. With `FAKE_COST_PUSH=1` it FIRST
//!   pushes TWO `cost_update` frames through its OWN
//!   `PI_ARCHIMEDES_BRIDGE_SOCKET` (one connection per push, each acked and
//!   read before the next — the desktop's `end_turn`-time capture is
//!   complete), then the chunk + `end_turn`.
//! - `subagent-hang` (`FAKE_SUBAGENT_MODE=hang`): `session/prompt` → one chunk
//!   then NEVER respond (the prompt stays open forever — the cancellation
//!   target).
//! - `subagent-ask` (`FAKE_SUBAGENT_MODE=ask`): `session/prompt` → connect to
//!   the agent's OWN `PI_ARCHIMEDES_BRIDGE_SOCKET`, send an `ask` request frame
//!   (`id: "fake-subagent-ask-1"`), read the response line, echo it as a chunk,
//!   then `end_turn` (the subagent's own bridge round-trip, the `ask` path).
//! - `subagent-echo` (`FAKE_SUBAGENT_MODE=echo`): `session/prompt` → one chunk
//!   with the PROMPT TEXT verbatim (a per-dispatch DISTINCT final text — the
//!   shared-capture regression), then `end_turn`; the `EMPTY` prompt text is a
//!   NO-TEXT turn (no chunks, `end_turn` only). An optional
//!   `FAKE_SUBAGENT_DELAY_MS` is honored (the "thinking" delay).
//!
//! The subagent's bridge env vars are set by the desktop; the fake agent
//! ignores them EXCEPT the `dispatch` / `dispatch-cancel` / `ask` modes and
//! the `FAKE_COST_PUSH` rule, which connect to `PI_ARCHIMEDES_BRIDGE_SOCKET`
//! (a bare Unix socket — the desktop's per-spawn peer-verified listener; in-test
//! the fake agent is a direct child of the test process, so peer verification
//! passes 1 hop).
//!
//! Wire-format notes: property keys are camelCase (`sessionUpdate`,
//! `agentCapabilities`, `messageId`, `optionId`); discriminator
//! values are snake_case (`agent_message_chunk`, `end_turn`, `allow_once`).

use std::io::{self, BufRead, Write};
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::process::ExitCode;

/// A bridge connection (a bare Unix socket on Unix). A named trait (with
/// `Read` + `Write` + `Send` as supertraits) keeps the object cross-platform-
/// compilable: on non-Unix platforms (the bridge is unavailable — fail-closed,
/// ADR 0003) `connect_bridge_socket` returns `Err`, and the `dispatch` /
/// `ask` modes echo a marker + `end_turn` (the prompt still resolves).
trait BridgeStream: std::io::Read + std::io::Write + Send {}
impl<T: std::io::Read + std::io::Write + Send> BridgeStream for T {}
type BridgeConn = Box<dyn BridgeStream>;

/// Connect to the bridge socket (a bare Unix socket — the desktop's per-spawn
/// peer-verified listener). On non-Unix platforms (the bridge is unavailable —
/// fail-closed, ADR 0003), this returns `Err` (the mode echoes a marker +
/// `end_turn`, so the prompt still resolves).
fn connect_bridge_socket(socket: &str) -> std::io::Result<BridgeConn> {
    #[cfg(unix)]
    {
        Ok(Box::new(UnixStream::connect(socket)?))
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

/// Whether the agent is running as the SUBAGENT (the `PI_ACP_PI_COMMAND` env
/// rule — the desktop sets it ONLY for subagent spawns, via the per-dispatch
/// launch wrapper). When set, the agent behaves as the subagent REGARDLESS of
/// the positional arg (the `args: ["dispatch"]` apply to both spawns).
fn is_subagent() -> bool {
    std::env::var("PI_ACP_PI_COMMAND").is_ok()
}

/// The session id this agent reports. The SUBAGENT reports a DISTINCT id so
/// its `session-update` / `session-closed` events are distinguishable from the
/// main's (`fake-session-1`, overridable via `FAKE_SESSION_ID`); in production
/// the pi session ids are unique.
///
/// `FAKE_SUBAGENT_SESSION_ID_PER_PID` (when `FAKE_SUBAGENT_SESSION_ID` is
/// unset) selects `fake-subagent-<pid>` — DISTINCT per process, so two
/// concurrent subagents spawned from the SAME registry entry report distinct
/// ACP session ids.
fn session_id(subagent: bool) -> String {
    if subagent {
        if let Ok(id) = std::env::var("FAKE_SUBAGENT_SESSION_ID") {
            return id;
        }
        if std::env::var("FAKE_SUBAGENT_SESSION_ID_PER_PID").is_ok() {
            return format!("fake-subagent-{}", std::process::id());
        }
        "fake-subagent-1".to_string()
    } else {
        std::env::var("FAKE_SESSION_ID").unwrap_or_else(|_| "fake-session-1".to_string())
    }
}

/// The subagent variant (`subagent` / `hang` / `ask`), from `FAKE_SUBAGENT_MODE`
/// (default `subagent`).
fn subagent_variant() -> String {
    std::env::var("FAKE_SUBAGENT_MODE").unwrap_or_else(|_| "subagent".to_string())
}

fn fake_config_options() -> serde_json::Value {
    serde_json::json!([
      { "type": "select", "id": "model", "category": "model", "name": "Model",
        "description": "Select the model for this session",
        "currentValue": "acme/alpha",
        "options": [
          { "value": "acme/alpha", "name": "acme/Alpha" },
          { "value": "acme/beta",  "name": "acme/Beta" },
          { "value": "acme/gamma", "name": "acme/Gamma" }
        ] },
      { "type": "select", "id": "thought_level", "category": "thought_level", "name": "Thinking",
        "description": "Set the reasoning effort for this session",
        "currentValue": "medium",
        "options": [
          { "value": "off", "name": "Thinking: off" },
          { "value": "minimal", "name": "Thinking: minimal" },
          { "value": "low", "name": "Thinking: low" },
          { "value": "medium", "name": "Thinking: medium" },
          { "value": "high", "name": "Thinking: high" },
          { "value": "xhigh", "name": "Thinking: xhigh" }
        ] }
    ])
}

fn main() -> ExitCode {
    let stdin = io::stdin();
    let stdout = io::stdout();

    let mut reader = stdin.lock();
    let mut out = stdout.lock();

    // Mode is the first positional argument (see the module docs), UNLESS the
    // `PI_ACP_PI_COMMAND` env rule selects the subagent behavior (the registry
    // entry's `args` apply to both the main and subagent spawns).
    let positional = std::env::args().nth(1).unwrap_or_default();
    let subagent = is_subagent();
    let mode = if subagent {
        "subagent".to_string()
    } else {
        positional.clone()
    };
    // The session id (DISTINCT for the subagent).
    let sid = session_id(subagent);

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
        let mut config_options = fake_config_options();

        match method {
            "session/set_config_option" => {
                if mode == "set_config_option_error" {
                    let error = serde_json::json!({
                        "code": -32602,
                        "message": "fake set config option failure",
                    });
                    write_error(&mut out, &id, &error);
                } else {
                    let params = frame.get("params").and_then(|p| p.as_object());
                    let config_id = params
                        .and_then(|p| p.get("configId"))
                        .and_then(|v| v.as_str());
                    let value = params.and_then(|p| p.get("value")).and_then(|v| v.as_str());

                    if let (Some(cid), Some(val)) = (config_id, value) {
                        let mut found = false;
                        if let Some(list) = config_options.as_array_mut() {
                            for item in list {
                                if item.get("id").and_then(|i| i.as_str()) == Some(cid) {
                                    if let Some(opts) =
                                        item.get_mut("options").and_then(|o| o.as_array_mut())
                                    {
                                        for opt in opts {
                                            if opt.get("value").and_then(|v| v.as_str())
                                                == Some(val)
                                            {
                                                item["currentValue"] = serde_json::json!(val);
                                                found = true;
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        if found {
                            write_config_update(&mut out, &sid, &config_options);
                            write_result(
                                &mut out,
                                &id,
                                &serde_json::json!({ "configOptions": config_options }),
                            );
                        } else {
                            let error = serde_json::json!({
                                "code": -32602,
                                "message": "unknown config option",
                            });
                            write_error(&mut out, &id, &error);
                        }
                    } else {
                        let error = serde_json::json!({
                            "code": -32602,
                            "message": "unknown config option",
                        });
                        write_error(&mut out, &id, &error);
                    }
                }
            }
            "initialize" => {
                // `loadSession` is advertised for the `resume` POSITIONAL mode
                // (the subagent never resumes — `mode` is `subagent` here).
                let load_session = positional == "resume";
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
                write_chunk(&mut out, &sid, "m1", "resumed");
                let result = serde_json::json!({
                    "sessionId": sid,
                    "configOptions": fake_config_options(),
                });
                write_result(&mut out, &id, &result);
            }
            "session/new" => {
                // The MAIN `hang` mode: never answer, to hold the client's
                // session/new request open forever. The SUBAGENT `hang`
                // variant hangs on the PROMPT instead (session/new is answered
                // here — `mode` is `subagent`, not `hang`).
                if mode != "hang" {
                    let result = serde_json::json!({
                        "sessionId": sid,
                        "configOptions": fake_config_options(),
                    });
                    write_result(&mut out, &id, &result);
                }
            }
            "session/prompt" => {
                if subagent {
                    handle_prompt_subagent(&mut out, &id, &sid, &subagent_variant(), &frame);
                } else {
                    match mode.as_str() {
                        "permission" => handle_prompt_permission(&mut reader, &mut out, &id, &sid),
                        "two-msgs" => handle_prompt_two_msgs(&mut out, &id, &sid),
                        "dispatch" => handle_prompt_dispatch(&mut out, &id, &sid, false),
                        "dispatch-cancel" => handle_prompt_dispatch(&mut out, &id, &sid, true),
                        "dispatch-two" => handle_prompt_dispatch_two(&mut out, &id, &sid),
                        _ => handle_prompt_default(&mut out, &id, &sid),
                    }
                }
            }
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
fn handle_prompt_default(out: &mut impl Write, prompt_id: &Option<serde_json::Value>, sid: &str) {
    write_chunk(out, sid, "m1", "hello");
    write_chunk(out, sid, "m1", " world");
    write_result(
        out,
        prompt_id,
        &serde_json::json!({ "stopReason": "end_turn" }),
    );
}

/// Two-messages mode: stream two chunks with DISTINCT `messageId`s
/// (`m1` → `"hello"`, then `m2` → `"world"`), then end the turn. Used by the
/// `text_capture` / `last_message_id` unit test (the "last message" is `m2`,
/// not derived from `HashMap` iteration order).
fn handle_prompt_two_msgs(out: &mut impl Write, prompt_id: &Option<serde_json::Value>, sid: &str) {
    write_chunk(out, sid, "m1", "hello");
    write_chunk(out, sid, "m2", "world");
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
    sid: &str,
) {
    // A chunk emitted BEFORE the permission request. If the client's event
    // loop is blocked while the permission handler is pending, this chunk
    // would never arrive — so its arrival proves the loop stayed responsive.
    write_chunk(out, sid, "m1", "pre");

    let perm_id = 100;
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
    write_chunk(out, sid, "m1", &format!("outcome:{outcome_text}"));

    write_chunk(out, sid, "m1", "hello");
    write_chunk(out, sid, "m1", " world");
    write_result(
        out,
        prompt_id,
        &serde_json::json!({ "stopReason": "end_turn" }),
    );
}

/// `dispatch` mode (the MAIN agent): connect to the bridge socket, send a
/// `dispatch_subagent` request frame (the `task` from `FAKE_DISPATCH_TASK`),
/// then — `cancel` = `false` — read the response line (blocking; the connection
/// stays open until the first data, the desktop's contract) and echo
/// `dispatch:<result.output>` (or `dispatch:<error>`); — `cancel` = `true` —
/// close the connection WITHOUT reading (the main agent's abort — the
/// connection is held open briefly first, so the subagent ESTABLISHES before
/// the abort cancels it) and echo a marker chunk. Either way, end the turn
/// (the main stays live).
fn handle_prompt_dispatch(
    out: &mut impl Write,
    prompt_id: &Option<serde_json::Value>,
    sid: &str,
    cancel: bool,
) {
    let socket = match std::env::var("PI_ARCHIMEDES_BRIDGE_SOCKET") {
        Ok(s) => s,
        Err(_) => {
            // No bridge socket (defensive): echo a marker and end the turn so
            // the prompt still resolves.
            write_chunk(out, sid, "m1", "dispatch:no-bridge");
            write_result(
                out,
                prompt_id,
                &serde_json::json!({ "stopReason": "end_turn" }),
            );
            return;
        }
    };
    let task = std::env::var("FAKE_DISPATCH_TASK").unwrap_or_default();
    let stream = match connect_bridge_socket(&socket) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("fake_agent: dispatch connect failed: {e}");
            write_chunk(out, sid, "m1", "dispatch:connect-error");
            write_result(
                out,
                prompt_id,
                &serde_json::json!({ "stopReason": "end_turn" }),
            );
            return;
        }
    };

    let frame = serde_json::json!({
        "v": 1,
        "type": "request",
        "id": "fake-dispatch-1",
        "method": "dispatch_subagent",
        "source": "main",
        "params": {
            "agentName": "fake",
            "task": task,
            "systemPrompt": null,
            "model": null,
            "thinking": null,
            "tools": null,
        },
    });
    let mut stream = stream;
    let data = frame.to_string() + "\n";
    if let Err(e) = stream.write_all(data.as_bytes()) {
        eprintln!("fake_agent: dispatch write failed: {e}");
        write_chunk(out, sid, "m1", "dispatch:write-error");
        write_result(
            out,
            prompt_id,
            &serde_json::json!({ "stopReason": "end_turn" }),
        );
        return;
    }
    let _ = stream.flush();

    if cancel {
        // Hold the bridge connection open briefly BEFORE aborting (a real
        // aborting main agent was WAITING for the response — its connection
        // stays open while it waits; the delay lets the subagent ESTABLISH
        // before the abort cancels it, so the E2E observes the
        // established-then-cancelled path).
        std::thread::sleep(std::time::Duration::from_millis(300));
        // Close WITHOUT reading (the main agent's abort — the parent connection
        // close cancels the in-flight dispatch). Dropping `stream` closes it.
        write_chunk(out, sid, "m1", "dispatch:aborted");
        write_result(
            out,
            prompt_id,
            &serde_json::json!({ "stopReason": "end_turn" }),
        );
        return;
    }

    // Read the response line (blocking; the connection stays open until the
    // first data).
    let mut line = String::new();
    let mut reader = io::BufReader::new(stream);
    match reader.read_line(&mut line) {
        Ok(0) => {
            // EOF before a response (the desktop closed — the dispatch was
            // cancelled on the desktop side).
            eprintln!("fake_agent[DEBUG]: dispatch read EOF");
            write_chunk(out, sid, "m1", "dispatch:cancelled");
        }
        Ok(_) => {
            eprintln!("fake_agent[DEBUG]: dispatch read response: {line}");
            let resp: serde_json::Value =
                serde_json::from_str(line.trim()).unwrap_or(serde_json::Value::Null);
            let echo = if let Some(output) = resp
                .get("result")
                .and_then(|r| r.get("output"))
                .and_then(serde_json::Value::as_str)
            {
                format!("dispatch:{output}")
            } else if let Some(err) = resp.get("error").and_then(serde_json::Value::as_str) {
                format!("dispatch:{err}")
            } else {
                "dispatch:unknown".to_string()
            };
            write_chunk(out, sid, "m1", &echo);
        }
        Err(e) => {
            eprintln!("fake_agent: dispatch read failed: {e}");
            write_chunk(out, sid, "m1", "dispatch:read-error");
        }
    }
    write_result(
        out,
        prompt_id,
        &serde_json::json!({ "stopReason": "end_turn" }),
    );
}

/// `subagent` mode: the subagent's `session/prompt`, selected by the variant
/// (`FAKE_SUBAGENT_MODE`): `subagent` → one chunk (`"subagent-done"`) +
/// `end_turn` (with `FAKE_COST_PUSH=1` it FIRST pushes TWO `cost_update`
/// frames through its OWN bridge — see `push_cost_updates`); `hang` → one
/// chunk then NEVER respond (the prompt stays open forever — the cancellation
/// target); `ask` → connect to the agent's OWN bridge, send an `ask`, read the
/// response, echo it as a chunk, then `end_turn` (the subagent's own bridge
/// round-trip, the `ask` path, no relay); `echo` → one chunk with the PROMPT
/// TEXT verbatim (a per-dispatch DISTINCT final text — the shared-capture
/// regression), then `end_turn` (the `EMPTY` prompt text is a NO-TEXT turn —
/// no chunks, `end_turn` only).
fn handle_prompt_subagent(
    out: &mut impl Write,
    prompt_id: &Option<serde_json::Value>,
    sid: &str,
    variant: &str,
    frame: &serde_json::Value,
) {
    match variant {
        "hang" => {
            // Answer with ONE chunk, then never respond (the prompt request stays
            // open forever — the cancellation target).
            write_chunk(out, sid, "m1", "subagent-hang");
        }
        "ask" => {
            // The subagent's own bridge round-trip: connect to its OWN
            // `PI_ARCHIMEDES_BRIDGE_SOCKET` (the desktop set it for the
            // subagent's spawn — the subagent's own listener, NOT the main's),
            // send an `ask`, read the response, echo it as a chunk, then end.
            ask_round_trip(out, sid);
            write_result(
                out,
                prompt_id,
                &serde_json::json!({ "stopReason": "end_turn" }),
            );
        }
        "echo" => {
            // The PROMPT TEXT verbatim (a per-dispatch DISTINCT final text —
            // the shared-capture regression); the `EMPTY` prompt text is a
            // NO-TEXT turn (no chunks, `end_turn` only). An optional
            // `FAKE_SUBAGENT_DELAY_MS` is honored (the "thinking" delay
            // widens the concurrent window).
            let delay_ms = std::env::var("FAKE_SUBAGENT_DELAY_MS")
                .ok()
                .and_then(|d| d.parse::<u64>().ok())
                .unwrap_or(0);
            if delay_ms > 0 {
                std::thread::sleep(std::time::Duration::from_millis(delay_ms));
            }
            let text = prompt_text(frame.get("params").unwrap_or(&serde_json::Value::Null));
            if text != "EMPTY" {
                write_chunk(out, sid, "m1", &text);
            }
            write_result(
                out,
                prompt_id,
                &serde_json::json!({ "stopReason": "end_turn" }),
            );
        }
        _ => {
            // Default (`subagent`): one chunk + end the turn. An optional
            // `FAKE_SUBAGENT_DELAY_MS` simulates the agent "thinking" before
            // responding (it extends the process's lifetime so the E2E can
            // observe the subagent as a separate process; the default of 0
            // / unset keeps the immediate-response behavior).
            let delay_ms = std::env::var("FAKE_SUBAGENT_DELAY_MS")
                .ok()
                .and_then(|d| d.parse::<u64>().ok())
                .unwrap_or(0);
            if delay_ms > 0 {
                std::thread::sleep(std::time::Duration::from_millis(delay_ms));
            }
            // `FAKE_COST_PUSH=1`: push TWO `cost_update` frames through the
            // agent's OWN bridge BEFORE answering the prompt (one connection
            // per push; the bare `ack` of each is read BEFORE the next write
            // and before the prompt answer, so the desktop's `end_turn`-time
            // capture is complete).
            if std::env::var("FAKE_COST_PUSH").is_ok() {
                push_cost_updates(out, sid);
            }
            write_chunk(out, sid, "m1", "subagent-done");
            write_result(
                out,
                prompt_id,
                &serde_json::json!({ "stopReason": "end_turn" }),
            );
        }
    }
}

/// The `ask` round-trip: connect to the agent's OWN `PI_ARCHIMEDES_BRIDGE_SOCKET`,
/// send an `ask` request frame (`id: "fake-subagent-ask-1"`), read the response
/// line, and echo it as a chunk (the desktop answers it via
/// `SubagentSessionManager::respond_bridge_request`; a timeout / close yields
/// the terminal `error:"cancelled"` frame).
fn ask_round_trip(out: &mut impl Write, sid: &str) {
    let socket = match std::env::var("PI_ARCHIMEDES_BRIDGE_SOCKET") {
        Ok(s) => s,
        Err(_) => {
            write_chunk(out, sid, "m1", "ask:no-bridge");
            return;
        }
    };
    let stream = match connect_bridge_socket(&socket) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("fake_agent: ask connect failed: {e}");
            write_chunk(out, sid, "m1", "ask:connect-error");
            return;
        }
    };

    let frame = serde_json::json!({
        "v": 1,
        "type": "request",
        "id": "fake-subagent-ask-1",
        "method": "ask",
        "source": "main",
        "params": { "question": "subagent-ask" },
    });
    let mut stream = stream;
    let data = frame.to_string() + "\n";
    if let Err(e) = stream.write_all(data.as_bytes()) {
        eprintln!("fake_agent: ask write failed: {e}");
        write_chunk(out, sid, "m1", "ask:write-error");
        return;
    }
    let _ = stream.flush();

    // Read the response line (blocking; the desktop holds the connection open
    // until it answers — `respond_bridge_request` — or the request is cancelled).
    let mut line = String::new();
    let mut reader = io::BufReader::new(stream);
    match reader.read_line(&mut line) {
        Ok(0) => {
            // EOF before a response (the request was cancelled / timed out).
            write_chunk(out, sid, "m1", "ask:cancelled");
        }
        Ok(_) => {
            // Echo the response line verbatim as a chunk.
            write_chunk(out, sid, "m1", line.trim());
        }
        Err(e) => {
            eprintln!("fake_agent: ask read failed: {e}");
            write_chunk(out, sid, "m1", "ask:read-error");
        }
    }
}

/// The `cost_update` push (the `FAKE_COST_PUSH` rule): push TWO `cost_update`
/// frames through the agent's OWN `PI_ARCHIMEDES_BRIDGE_SOCKET` (the desktop
/// set it for the subagent's spawn — the subagent's own listener, NOT the
/// main's). ONE connection PER push (the desktop reads exactly ONE frame per
/// connection), and the bare `ack` line of each push is read (blocking) BEFORE
/// the next write and before the caller answers the prompt — a push is ACKED,
/// not responded-to (the read is the bare line `ack`, NOT JSON — do not
/// parse it), so the desktop has captured both pushes before the caller's
/// `end_turn` triggers the desktop's metrics snapshot. `seq` is OMITTED
/// (`seq == 0` is always delivered — the desktop's dedupe drops `seq <=
/// last_seq` only for a non-zero `seq`).
fn push_cost_updates(out: &mut impl Write, sid: &str) {
    let socket = match std::env::var("PI_ARCHIMEDES_BRIDGE_SOCKET") {
        Ok(s) => s,
        Err(_) => {
            // No bridge socket (defensive): echo a marker (the prompt still
            // resolves; the desktop's metrics stay the zeros).
            write_chunk(out, sid, "m1", "cost:no-bridge");
            return;
        }
    };
    // The two per-turn deltas (payload 1 has `cacheReadTokens` /
    // `cacheWriteTokens` ABSENT — the desktop's accumulator treats absent as
    // 0; the desktop's accumulator must SUM them: input 300, output 75,
    // cost 0.003).
    let payloads = [
        serde_json::json!({
            "v": 1,
            "type": "push",
            "event": "cost_update",
            "payload": { "source": "main", "inputTokens": 100, "outputTokens": 50, "cost": 0.001 }
        }),
        serde_json::json!({
            "v": 1,
            "type": "push",
            "event": "cost_update",
            "payload": { "source": "main", "inputTokens": 200, "outputTokens": 25, "cacheReadTokens": 10, "cost": 0.002 }
        }),
    ];
    for payload in payloads {
        let stream = match connect_bridge_socket(&socket) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("fake_agent: cost-push connect failed: {e}");
                write_chunk(out, sid, "m1", "cost:connect-error");
                return;
            }
        };
        let mut stream = stream;
        let data = payload.to_string() + "\n";
        if let Err(e) = stream.write_all(data.as_bytes()) {
            eprintln!("fake_agent: cost-push write failed: {e}");
            write_chunk(out, sid, "m1", "cost:write-error");
            return;
        }
        let _ = stream.flush();
        // Read the bare `ack` line (blocking; the desktop acks AFTER the
        // capture — the line is NOT JSON, so do NOT parse it).
        let mut line = String::new();
        let mut reader = io::BufReader::new(&mut *stream);
        match reader.read_line(&mut line) {
            Ok(0) => {
                // EOF before an ack (the desktop closed — defensive).
                eprintln!("fake_agent: cost-push read EOF (no ack)");
                write_chunk(out, sid, "m1", "cost:no-ack");
                return;
            }
            Ok(_) => {
                if line.trim() != "ack" {
                    eprintln!("fake_agent: cost-push unexpected read: {line}");
                }
            }
            Err(e) => {
                eprintln!("fake_agent: cost-push read failed: {e}");
                write_chunk(out, sid, "m1", "cost:read-error");
                return;
            }
        }
        // Drop `stream` → close the connection (the desktop closes on the
        // first data; the next push opens a FRESH connection).
    }
}

/// `dispatch-two` mode (the MAIN agent, the shared-capture regression test):
/// send TWO `dispatch_subagent` frames (the tasks from `FAKE_DISPATCH_TASK` /
/// `FAKE_DISPATCH_TASK_2`) and echo each response (`dispatch1:<output-or-
/// error>`, `dispatch2:<output-or-error>`), then end the turn (the main stays
/// live).
///
/// CONCURRENT by default: both frames are written BEFORE either response is
/// read (the desktop dispatches the two subagents concurrently). `FAKE_
/// DISPATCH_TWO_SEQUENTIAL=1` reads response 1 BEFORE sending frame 2 (the
/// stale-carry-over shape: a no-text dispatch AFTER a text one).
fn handle_prompt_dispatch_two(
    out: &mut impl Write,
    prompt_id: &Option<serde_json::Value>,
    sid: &str,
) {
    let socket = match std::env::var("PI_ARCHIMEDES_BRIDGE_SOCKET") {
        Ok(s) => s,
        Err(_) => {
            // No bridge socket (defensive): echo markers and end the turn so
            // the prompt still resolves.
            write_chunk(out, sid, "m1", "dispatch1:no-bridge");
            write_chunk(out, sid, "m1", "dispatch2:no-bridge");
            write_result(
                out,
                prompt_id,
                &serde_json::json!({ "stopReason": "end_turn" }),
            );
            return;
        }
    };
    let task1 = std::env::var("FAKE_DISPATCH_TASK").unwrap_or_else(|_| "task-one".to_string());
    let task2 = std::env::var("FAKE_DISPATCH_TASK_2").unwrap_or_else(|_| "task-two".to_string());
    let sequential = std::env::var("FAKE_DISPATCH_TWO_SEQUENTIAL").is_ok();

    // Frame 1.
    let mut c1 = match send_dispatch_frame(&socket, "fake-dispatch-1", &task1) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("fake_agent: dispatch-two connect failed: {e}");
            write_chunk(out, sid, "m1", "dispatch1:connect-error");
            write_chunk(out, sid, "m1", "dispatch2:connect-error");
            write_result(
                out,
                prompt_id,
                &serde_json::json!({ "stopReason": "end_turn" }),
            );
            return;
        }
    };
    if sequential {
        // Read response 1 BEFORE sending frame 2 (the stale-carry-over shape:
        // the no-text dispatch runs AFTER the text dispatch completes).
        let echo1 = read_dispatch_echo(&mut c1, "dispatch1");
        write_chunk(out, sid, "m1", &echo1);
        let mut c2 = match send_dispatch_frame(&socket, "fake-dispatch-2", &task2) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("fake_agent: dispatch-two connect failed: {e}");
                write_chunk(out, sid, "m1", "dispatch2:connect-error");
                write_result(
                    out,
                    prompt_id,
                    &serde_json::json!({ "stopReason": "end_turn" }),
                );
                return;
            }
        };
        let echo2 = read_dispatch_echo(&mut c2, "dispatch2");
        write_chunk(out, sid, "m1", &echo2);
    } else {
        // Frame 2 BEFORE reading either response (the two subagents run
        // concurrently on the desktop's worker runtime).
        let mut c2 = match send_dispatch_frame(&socket, "fake-dispatch-2", &task2) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("fake_agent: dispatch-two connect failed: {e}");
                write_chunk(out, sid, "m1", "dispatch2:connect-error");
                write_result(
                    out,
                    prompt_id,
                    &serde_json::json!({ "stopReason": "end_turn" }),
                );
                return;
            }
        };
        let echo1 = read_dispatch_echo(&mut c1, "dispatch1");
        write_chunk(out, sid, "m1", &echo1);
        let echo2 = read_dispatch_echo(&mut c2, "dispatch2");
        write_chunk(out, sid, "m1", &echo2);
    }
    write_result(
        out,
        prompt_id,
        &serde_json::json!({ "stopReason": "end_turn" }),
    );
}

/// Connect to the bridge socket and write ONE `dispatch_subagent` request
/// frame (the connection stays open until the desktop's response line — the
/// desktop's contract; the returned connection is the open read end).
fn send_dispatch_frame(socket: &str, id: &str, task: &str) -> std::io::Result<BridgeConn> {
    let stream = connect_bridge_socket(socket)?;
    let frame = serde_json::json!({
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
            "tools": null,
        },
    });
    let mut stream = stream;
    let data = frame.to_string() + "\n";
    stream.write_all(data.as_bytes())?;
    stream.flush()?;
    Ok(stream)
}

/// Read the response line from a dispatch connection and format the echo
/// text: `dispatchN:<result.output>` / `dispatchN:<error>` / `dispatchN:
/// cancelled` (EOF — the dispatch was cancelled on the desktop side) /
/// `dispatchN:read-error`.
fn read_dispatch_echo(stream: &mut BridgeConn, prefix: &str) -> String {
    let mut line = String::new();
    let mut reader = io::BufReader::new(&mut **stream);
    match reader.read_line(&mut line) {
        Ok(0) => format!("{prefix}:cancelled"),
        Ok(_) => {
            eprintln!("fake_agent[DEBUG]: dispatch-two read response: {line}");
            let resp: serde_json::Value =
                serde_json::from_str(line.trim()).unwrap_or(serde_json::Value::Null);
            if let Some(output) = resp
                .get("result")
                .and_then(|r| r.get("output"))
                .and_then(serde_json::Value::as_str)
            {
                format!("{prefix}:{output}")
            } else if let Some(err) = resp.get("error").and_then(serde_json::Value::as_str) {
                format!("{prefix}:{err}")
            } else {
                format!("{prefix}:unknown")
            }
        }
        Err(e) => {
            eprintln!("fake_agent: dispatch-two read failed: {e}");
            format!("{prefix}:read-error")
        }
    }
}

/// The prompt text of a `session/prompt` frame (all text content blocks
/// concatenated; `""` when the frame carries no text).
fn prompt_text(params: &serde_json::Value) -> String {
    params
        .get("prompt")
        .and_then(|p| p.as_array())
        .map(|blocks| {
            blocks
                .iter()
                .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
                .filter_map(|b| b.get("text").and_then(|t| t.as_str()).map(str::to_string))
                .collect::<String>()
        })
        .unwrap_or_default()
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
/// `sid` is the session id (the subagent's DISTINCT id in subagent mode).
fn write_chunk(w: &mut impl Write, sid: &str, message_id: &str, text: &str) {
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

/// Write a JSON-RPC `session/update` notification with a
/// config_option_update.
fn write_config_update(w: &mut impl Write, sid: &str, config: &serde_json::Value) {
    let frame = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {
            "sessionId": sid,
            "update": {
                "sessionUpdate": "config_option_update",
                "configOptions": config,
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
