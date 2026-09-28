//! The agent registry (native-agent-harness Task 7): the `kind: external |
//! native` discriminator, the `#[serde(default)]` `command` (a `kind: native`
//! entry carries a harness config, NOT a spawn spec), and the built-in
//! 'Archimedes' native agent entry merged into the loaded registry (appended
//! AFTER the user entries — `NewSpaceDialog` uses `agents[0]` as the default,
//! so the merge must never reorder / prepend).

use std::path::PathBuf;

use archimedes_lib::config::{AgentEntry, AgentKind, Registry};
use serde_json::json;

fn temp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("registry-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A user `agents.json` with a single `pi` entry (NO `kind` — the pre-Task-7
/// shape; it must parse and default to `kind: external`).
fn write_pi_only(dir: &std::path::Path) {
    std::fs::write(
        dir.join("agents.json"),
        json!({
            "agents": [{
                "id": "pi",
                "name": "Pi",
                "command": "pi",
                "args": ["--mode", "rpc"],
                "env": {},
                "bridge": true,
            }]
        })
        .to_string(),
    )
    .unwrap();
}

/// (a) A loaded registry (pi only, from a temp `agents.json`) gets the
/// built-in `archimedes` entry APPENDED after the user entries — the user
/// order is preserved (`agents[0]` is still the user's first entry, NOT the
/// built-in: a prepended built-in would flip the frontend default to native).
#[test]
fn load_appends_the_builtin_after_the_user_entries() {
    let dir = temp_dir();
    write_pi_only(&dir);
    let registry = Registry::load(&dir).unwrap();

    assert_eq!(registry.agents.len(), 2, "pi + the built-in archimedes");
    assert_eq!(registry.agents[0].id, "pi", "the user entry stays first");
    assert_eq!(
        registry.agents[1].id, "archimedes",
        "the built-in is appended"
    );
    // The user entry is UNCHANGED in shape (no `kind` on the wire →
    // `external`).
    assert_eq!(registry.agents[0].kind, AgentKind::External);
    assert_eq!(registry.agents[0].command, "pi");
    // The built-in: a harness config, NOT a spawn spec.
    let builtin = &registry.agents[1];
    assert_eq!(builtin.kind, AgentKind::Native);
    assert!(
        builtin.command.is_empty(),
        "a native entry has no spawn spec"
    );
    let harness = builtin
        .harness
        .as_ref()
        .expect("the built-in carries a harness config");
    assert_eq!(harness.provider, "openai-compatible");
    // `default_model` is `None` in the built-in (resolved from the
    // `ModelCatalog` at session start — `Registry::load` is pure serde and
    // NOT coupled to `seed_from_pi_config()`).
    assert_eq!(harness.default_model, None);
    assert_eq!(harness.default_thinking_level.as_deref(), Some("high"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// (b) A user override of the built-in's `id` wins (the built-in is NOT
/// appended a second time).
#[test]
fn a_user_override_of_the_builtin_wins() {
    let dir = temp_dir();
    std::fs::write(
        dir.join("agents.json"),
        json!({
            "agents": [
                {
                    "id": "pi",
                    "name": "Pi",
                    "command": "pi",
                    "args": ["--mode", "rpc"],
                },
                {
                    "id": "archimedes",
                    "name": "My Archimedes",
                    "kind": "native",
                    "harness": { "provider": "openai-compatible", "default_model": "tama/custom" },
                },
            ]
        })
        .to_string(),
    )
    .unwrap();
    let registry = Registry::load(&dir).unwrap();
    assert_eq!(
        registry.agents.len(),
        2,
        "exactly ONE archimedes (the user's)"
    );
    let mine = registry.get("archimedes").unwrap();
    assert_eq!(mine.name, "My Archimedes");
    assert_eq!(
        mine.harness
            .as_ref()
            .and_then(|h| h.default_model.as_deref()),
        Some("tama/custom"),
        "the user override's harness wins"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// (c) A `kind: native` entry with `command` omitted parses (`command`
/// defaults to `""`, `harness` defaults to `None`).
#[test]
fn a_native_entry_without_command_parses() {
    let entry: AgentEntry =
        serde_json::from_str(r#"{ "id": "n", "name": "N", "kind": "native" }"#).unwrap();
    assert_eq!(entry.kind, AgentKind::Native);
    assert_eq!(entry.command, "", "command defaults to empty");
    assert_eq!(entry.harness, None, "harness defaults to None");
}

/// (d) The pre-Task-7 entry shape (no `kind` key) is UNCHANGED: `kind`
/// defaults to `external`.
#[test]
fn the_existing_entry_shape_defaults_to_external() {
    let entry: AgentEntry =
        serde_json::from_str(r#"{ "id": "pi", "name": "Pi", "command": "pi" }"#).unwrap();
    assert_eq!(entry.kind, AgentKind::External);
    assert_eq!(entry.command, "pi");
    assert_eq!(entry.harness, None);
}

/// (e) A MISSING `agents.json` (the default registry) also gets the built-in
/// appended AFTER `pi` (a fresh install sees both; `pi` stays the default).
#[test]
fn the_default_registry_gets_the_builtin_appended() {
    let dir = temp_dir();
    let registry = Registry::load(&dir).unwrap();
    assert_eq!(registry.agents.len(), 2);
    assert_eq!(registry.agents[0].id, "pi", "pi stays the default (first)");
    assert_eq!(registry.agents[1].id, "archimedes");
    let _ = std::fs::remove_dir_all(&dir);
}
