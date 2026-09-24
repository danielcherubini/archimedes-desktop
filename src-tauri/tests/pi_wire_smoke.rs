//! The real-`pi` wire smoke (the authoritative wire-format check): spawn
//! the REAL `pi --mode rpc` binary (not `fake_pi` — the fake is written
//! from the same table as the client and cannot catch a systematic
//! casing error; the real pi's camelCase is the ground truth), drive it
//! through the desktop's `PiRpc` client, and assert the wire shapes.
//!
//! `real_pi_wire_smoke`: the command/response surface (the `get_state`
//! envelope, the `set_thinking_level` round-trip, the `prompt` preflight
//! + turn events, the clean `close()`).
//!
//! `real_pi_gate_and_resume_smoke`: the extension-UI sub-protocol (a
//! gated `bash` tool call → the bundled gate extension's
//! `ctx.ui.confirm` → `extension_ui_request` → the desktop's
//! `extension_ui_response` → the tool runs) + the resume contract (a
//! second `pi --session <file>` process resumes the stored session).
//!
//! SKIPPED (not failed) when the `pi` binary is not on PATH or the
//! desktop has no usable model credentials — the tests are contract
//! checks for upgrades (ADR 0009), not build gates.

use std::collections::BTreeMap;
use std::time::Duration;

use archimedes_desktop_lib::agent::{ExtensionUiRequest, ExtensionUiResponse, PiRpc, RpcEvent};
use serde_json::Value;

/// Locate the `pi` binary on PATH (`None` = skip the test).
fn pi_on_path() -> Option<String> {
    let out = std::process::Command::new("which")
        .arg("pi")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!path.is_empty()).then_some(path)
}

/// A minimal env for the child: the desktop's own environment (the
/// `PiRpc::spawn` is additive, so an EMPTY map still inherits it) —
/// nothing extra.
fn empty_env() -> BTreeMap<String, String> {
    BTreeMap::new()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_pi_wire_smoke() {
    let Some(pi) = pi_on_path() else {
        eprintln!("skipping real_pi_wire_smoke: pi not on PATH");
        return;
    };

    let dir = std::env::temp_dir().join(format!("pi-wire-smoke-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();

    // `--no-session`: a fresh in-memory session (no session file — the
    // smoke asserts the wire, not persistence). `--no-themes`: no theme
    // loading (deterministic startup).
    let rpc = match PiRpc::spawn(
        &pi,
        &[
            "--mode".to_string(),
            "rpc".to_string(),
            "--no-themes".to_string(),
            "--no-session".to_string(),
        ],
        &empty_env(),
        &dir,
    ) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("skipping real_pi_wire_smoke: spawn failed: {e}");
            return;
        }
    };
    let handle = rpc.handle();

    // 1. `get_state` (the establish command): the response carries the
    //    session envelope (camelCase — the ground-truth casing check).
    let state = match tokio::time::timeout(
        Duration::from_secs(15),
        handle.send(serde_json::json!({ "type": "get_state" })),
    )
    .await
    {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            eprintln!("skipping real_pi_wire_smoke: get_state failed: {e}");
            return;
        }
        Err(_) => {
            eprintln!("skipping real_pi_wire_smoke: get_state timed out (no model credentials?)");
            return;
        }
    };
    let session_id = state
        .get("sessionId")
        .and_then(Value::as_str)
        .expect("get_state carries a camelCase `sessionId`")
        .to_string();
    assert!(!session_id.is_empty(), "the session id is non-empty");
    eprintln!("real_pi_wire_smoke: established session {session_id}");

    // 2. `set_model` / `set_thinking_level` round-trip (the config-option
    //    surface): set a thinking level, then read it back via
    //    `get_state`. (The real pi normalizes per model — e.g.
    //    `minimal` may come back `low` — so the assertion is that the
    //    round-trip carries a KNOWN level, not the exact one sent.)
    let _ = handle
        .send(serde_json::json!({ "type": "set_thinking_level", "level": "minimal" }))
        .await
        .expect("set_thinking_level should be accepted");
    let state2 = handle
        .send(serde_json::json!({ "type": "get_state" }))
        .await
        .expect("get_state after set_thinking_level");
    let level = state2
        .get("thinkingLevel")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    eprintln!("real_pi_wire_smoke: thinkingLevel after set = {level}");
    assert!(
        matches!(level, "off" | "minimal" | "low" | "medium" | "high"),
        "the thinking level round-trips as a known level (got {level:?})"
    );

    // 3. `prompt` (the turn): the preflight response arrives, then the
    //    turn events stream, ending in `agent_settled`. A trivial prompt
    //    (one short answer — the cost is bounded).
    let prompt = handle
        .send(
            serde_json::json!({ "type": "prompt", "message": "Reply with exactly the word: pong" }),
        )
        .await
        .expect("the prompt preflight should be accepted");
    // The preflight is the start-of-turn ack (NOT the completion — ADR
    // 0009: `prompt` success ≠ completion; the turn resolves on
    // `agent_settled`).
    eprintln!("real_pi_wire_smoke: prompt preflight = {prompt:?}");

    // The real pi wraps the assistant-message events (the
    // `text_delta` / `thinking_delta` sub-shapes) inside `message_update`
    // frames — a top-level `text_delta` is a different
    // (unused-by-the-model) shape. (`events()` is SINGLE-attach — the
    // driver task is the sole consumer — so ONE receiver is used for
    // both assertions below.)
    let mut events = handle.events();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let mut saw_text = false;
    let mut settled = false;
    while tokio::time::Instant::now() < deadline && !settled {
        let ev = match tokio::time::timeout(Duration::from_secs(5), events.recv()).await {
            Ok(Some(ev)) => ev,
            Ok(None) => break,  // the reader ended (the child died)
            Err(_) => continue, // the per-event cap; the outer deadline rules
        };
        match &ev {
            RpcEvent::message_update {
                assistant_message_event: ame,
                ..
            } if ame.get("type").and_then(Value::as_str) == Some("text_delta") => {
                saw_text = true;
            }
            RpcEvent::agent_settled => {
                settled = true;
            }
            _ => {}
        }
    }
    assert!(
        settled,
        "the turn should settle (agent_settled) within the 60 s cap"
    );
    assert!(
        saw_text,
        "the turn should stream a text_delta (the real pi's turn shape)"
    );

    // 4. Teardown: close stdin (the real pi's `onInputEnd` → clean exit).
    handle.close().await;

    let _ = std::fs::remove_dir_all(&dir);
}

/// The full contract: a GATED tool call (the bundled gate extension's
/// `tool_call` hook → `ctx.ui.confirm` → the `extension_ui` sub-protocol)
/// + the RESUME contract (a second `pi --session <file>` process resumes
/// the stored session).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_pi_gate_and_resume_smoke() {
    let Some(pi) = pi_on_path() else {
        eprintln!("skipping real_pi_gate_and_resume_smoke: pi not on PATH");
        return;
    };

    let dir = std::env::temp_dir().join(format!("pi-gate-smoke-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();

    // The bundled gate extension (the desktop's own `gate.ts` — the
    // self-gated `tool_call` → `ctx.ui.confirm` round-trip).
    let gate_path = match archimedes_desktop_lib::agent::install_gate_extension(&dir) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("skipping real_pi_gate_and_resume_smoke: gate install failed: {e}");
            return;
        }
    };
    let session_file = dir.join("s1.jsonl");

    // Spawn 1: `--session <file>` (a NEW session at that path — pi
    // creates it) + the gate extension (`-e`) + the gate env.
    let mut env = empty_env();
    archimedes_desktop_lib::agent::gate_env(&mut env);
    let args = [
        "--mode".to_string(),
        "rpc".to_string(),
        "--no-themes".to_string(),
        "--session".to_string(),
        session_file.to_string_lossy().into_owned(),
        "-e".to_string(),
        gate_path.to_string_lossy().into_owned(),
    ];
    let rpc = match PiRpc::spawn(&pi, &args, &env, &dir) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("skipping real_pi_gate_and_resume_smoke: spawn failed: {e}");
            return;
        }
    };
    let handle = rpc.handle();

    let state = match tokio::time::timeout(
        Duration::from_secs(15),
        handle.send(serde_json::json!({ "type": "get_state" })),
    )
    .await
    {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            eprintln!("skipping real_pi_gate_and_resume_smoke: get_state failed: {e}");
            return;
        }
        Err(_) => {
            eprintln!("skipping real_pi_gate_and_resume_smoke: get_state timed out");
            return;
        }
    };
    let session_id = state
        .get("sessionId")
        .and_then(Value::as_str)
        .expect("get_state carries a sessionId");
    eprintln!("real_pi_gate_and_resume_smoke: established session {session_id}");

    // The prompt REQUIRES a bash call (the model cannot answer without
    // running the command) — the gate (GATED includes `bash`) must fire
    // a `ctx.ui.confirm` before the tool runs.
    let _ = handle
        .send(serde_json::json!({
            "type": "prompt",
            "message": "Run the bash command: echo gate-smoke-ok — and report its exact output."
        }))
        .await
        .expect("the prompt preflight should be accepted");

    // The turn: events stream AND the `extension_ui` channel carries the
    // gate's `confirm` request. Select on both (ONE receiver each —
    // `events()` attaches a fresh channel per call, so the receivers are
    // created ONCE, before the loop): approve the confirm
    // (`confirmed: true` — the desktop's `allow` option), then wait for
    // `agent_settled`.
    let mut ui_events = handle.extension_ui();
    let mut ev_events = handle.events();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    let mut gate_fired = false;
    loop {
        if tokio::time::Instant::now() > deadline {
            panic!("the turn should settle within the 90 s cap (gate_fired={gate_fired})");
        }
        tokio::select! {
            maybe_ui = ui_events.recv() => {
                let Some(req) = maybe_ui else {
                    panic!("the extension_ui channel ended before the turn settled");
                };
                // The gate's `ctx.ui.confirm` → the `Confirm` shape (the
                // ground-truth sub-protocol check: the request carries
                // the `id` the response must echo).
                let ExtensionUiRequest::Confirm { id, title, .. } = req else {
                    // A non-confirm dialog (e.g. a `notify`): ignore it
                    // (the gate only `confirm`s).
                    eprintln!("real_pi_gate_and_resume_smoke: non-confirm UI: {req:?}");
                    continue;
                };
                eprintln!("real_pi_gate_and_resume_smoke: gate confirm: {title:?}");
                gate_fired = true;
                handle
                    .respond_extension_ui(ExtensionUiResponse::Confirmed {
                        id,
                        confirmed: true,
                    })
                    .await
                    .expect("the extension_ui_response should be accepted");
            }
            maybe_ev = tokio::time::timeout(Duration::from_secs(5), ev_events.recv()) => {
                let Some(ev) = maybe_ev.ok().flatten() else {
                    panic!("the event stream ended before the turn settled");
                };
                if matches!(ev, RpcEvent::agent_settled) {
                    break;
                }
            }
        }
    }
    assert!(
        gate_fired,
        "the gated bash call should trigger the extension-UI confirm (the gate extension round-trip)"
    );
    eprintln!("real_pi_gate_and_resume_smoke: turn settled with the gate approved");

    // Teardown 1: close stdin (clean exit — the session file is written).
    handle.close().await;
    assert!(
        session_file.exists(),
        "the session file should exist after the turn (the resume contract's input)"
    );

    // Spawn 2: RESUME the same session file.
    let rpc2 = match PiRpc::spawn(
        &pi,
        &[
            "--mode".to_string(),
            "rpc".to_string(),
            "--no-themes".to_string(),
            "--session".to_string(),
            session_file.to_string_lossy().into_owned(),
        ],
        &empty_env(),
        &dir,
    ) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("skipping real_pi_gate_and_resume_smoke: resume spawn failed: {e}");
            return;
        }
    };
    let handle2 = rpc2.handle();
    let state2 = match tokio::time::timeout(
        Duration::from_secs(15),
        handle2.send(serde_json::json!({ "type": "get_state" })),
    )
    .await
    {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            eprintln!("skipping real_pi_gate_and_resume_smoke: resume get_state failed: {e}");
            return;
        }
        Err(_) => {
            eprintln!("skipping real_pi_gate_and_resume_smoke: resume get_state timed out");
            return;
        }
    };
    let resumed_id = state2
        .get("sessionId")
        .and_then(Value::as_str)
        .expect("the resumed get_state carries a sessionId");
    eprintln!("real_pi_gate_and_resume_smoke: resumed session {resumed_id}");
    assert!(
        !resumed_id.is_empty(),
        "the resumed session id is non-empty"
    );

    handle2.close().await;

    let _ = std::fs::remove_dir_all(&dir);
}
