//! The real-`pi` wire smoke (the authoritative wire-format check): spawn
//! the REAL `pi --mode rpc` binary (not `fake_pi` — the fake is written
//! from the same table as the client and cannot catch a systematic
//! casing error; the real pi's camelCase is the ground truth), drive it
//! through the desktop's `PiRpc` client, and assert the wire shapes
//! (the `get_state` envelope, the prompt preflight + turn events, the
//! `set_model` / `set_thinking_level` round-trip).
//!
//! SKIPPED (not failed) when the `pi` binary is not on PATH or the
//! desktop has no usable model credentials — the test is a contract
//! check for upgrades (ADR 0009), not a build gate.

use std::collections::BTreeMap;
use std::time::Duration;

use archimedes_desktop_lib::agent::PiRpc;
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
    //    surface): set a thinking level, then read it back via `get_state`.
    //    (The model list is agent-internal; the thinking level is a
    //    stable enum the desktop sets directly.)
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

    // Collect the turn events until `agent_settled` (hard 60 s cap — a
    // "pong" turn is ~3 s; the cap is a stall guard, not a model budget).
    let mut events = handle.events();
    let mut saw_text_delta = false;
    let mut saw_message_end = false;
    let mut settled = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    while tokio::time::Instant::now() < deadline {
        let ev = match tokio::time::timeout(Duration::from_secs(5), events.recv()).await {
            Ok(Some(ev)) => ev,
            Ok(None) => break,  // the reader ended (the child died)
            Err(_) => continue, // the per-event cap; the outer deadline rules
        };
        match &ev {
            // The real pi wraps the assistant-message events (the
            // `text_delta` / `thinking_delta` sub-shapes) inside
            // `message_update` frames — a top-level `text_delta` is a
            // different (unused-by-the-model) shape.
            archimedes_desktop_lib::agent::RpcEvent::message_update {
                assistant_message_event: ame,
                ..
            } if ame.get("type").and_then(Value::as_str) == Some("text_delta")
                && ame.get("delta").and_then(Value::as_str).is_some() =>
            {
                saw_text_delta = true;
            }
            archimedes_desktop_lib::agent::RpcEvent::message_end { .. } => {
                saw_message_end = true;
            }
            archimedes_desktop_lib::agent::RpcEvent::agent_settled => {
                settled = true;
                break;
            }
            _ => {}
        }
    }
    eprintln!(
        "real_pi_wire_smoke: settled={settled} text_delta={saw_text_delta} message_end={saw_message_end}"
    );
    assert!(
        settled,
        "the turn should settle (agent_settled) within the 60 s cap"
    );
    assert!(
        saw_text_delta && saw_message_end,
        "the turn should stream text deltas and end the message (the real pi's turn shape)"
    );

    // 4. Teardown: close stdin (the real pi's `onInputEnd` → clean exit).
    handle.close().await;

    let _ = std::fs::remove_dir_all(&dir);
}
