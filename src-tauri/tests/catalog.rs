//! `ModelCatalog` + config seeding (native-agent-harness Task 5, ADR
//! 0012): the desktop seeds its model catalog + auth from the user's
//! existing pi config files — best-effort (a missing/unparseable file
//! degrades to "no seeded model", not a crash).

use std::io::Write;
use std::path::Path;

use archimedes_lib::agent::harness::ModelCatalog;

/// Write one file into `dir` (creating it).
fn write_file(dir: &Path, name: &str, contents: &str) {
    let mut f = std::fs::File::create(dir.join(name)).unwrap();
    f.write_all(contents.as_bytes()).unwrap();
}

/// The fixture with the **REAL** pi config shapes (verified against
/// `~/.pi/agent/`): `settings.json` (a **bare** `defaultModel` +
/// `defaultProvider` + `enabledModels` + `compaction`), `auth.json`
/// (`provider → { key, type }` — NOT `{ api_key }`), and
/// `models-store.json` (`provider → { models, checkedAt, etag?,
/// lastModified? }` — including a sparse `tama`-like provider entry
/// with only `{ models, checkedAt }` (no `etag`/`lastModified`) and a
/// model entry with no `inputLimits`, to prove the sparse shape
/// parses).
fn write_full_fixture(dir: &Path) {
    write_file(
        dir,
        "settings.json",
        r#"{
  "lastChangelogVersion": "0.87.1",
  "defaultProvider": "tama",
  "defaultModel": "Qwen/Qwen3.8-27B",
  "packages": ["npm:pi-updater"],
  "defaultThinkingLevel": "high",
  "theme": "dracula",
  "compaction": {
    "enabled": true,
    "reserveTokens": 8192,
    "keepRecentTokens": 40000
  },
  "enabledModels": [
    "openrouter/moonshotai/kimi-k3",
    "tama/Qwen/Qwen3.8-27B",
    "google/gemini-2.5-flash",
    "tama/Unknown-Model"
  ]
}"#,
    );
    // `auth.json`: `provider → { key, type }` — `tama` is NOT in it
    // (a local gateway needing no key).
    write_file(
        dir,
        "auth.json",
        r#"{
  "google": { "type": "api_key", "key": "gk_123" },
  "eurouter": { "type": "api_key", "key": "eur_456" }
}"#,
    );
    // `models-store.json`: `openrouter`/`google` carry all four
    // provider-level fields; `tama` is sparse (only `{ models,
    // checkedAt }`) and its model entry has no `inputLimits`.
    write_file(
        dir,
        "models-store.json",
        r#"{
  "openrouter": {
    "models": [
      {
        "type": "chat",
        "id": "moonshotai/kimi-k3",
        "name": "MoonshotAI: Kimi K3",
        "api": "openai-completions",
        "baseUrl": "https://openrouter.ai/api/v1",
        "provider": "openrouter",
        "reasoning": true,
        "thinkingLevelMap": { "off": null, "low": "low", "medium": null, "high": "high", "max": "max" },
        "input": ["text", "image"],
        "cost": { "input": 0.8845, "output": 10.5346, "cacheRead": 0.33, "cacheWrite": 0 },
        "contextWindow": 1048576,
        "maxTokens": 131072,
        "compat": { "supportsDeveloperRole": false },
        "inputLimits": { "maxRequestBytes": 20971520 }
      }
    ],
    "checkedAt": 1790536933728,
    "lastModified": 1790239043000,
    "etag": "W/\"abc\""
  },
  "google": {
    "models": [
      {
        "id": "gemini-2.5-flash",
        "name": "Gemini 2.5 Flash",
        "api": "google-generative-ai",
        "baseUrl": "https://generativelanguage.googleapis.com/v1beta",
        "provider": "google",
        "reasoning": true,
        "input": ["text", "image"],
        "cost": { "input": 0.3, "output": 2.5, "cacheRead": 0.03, "cacheWrite": 0 },
        "contextWindow": 1048576,
        "maxTokens": 65536,
        "inputLimits": { "maxRequestBytes": 20971520 },
        "type": "chat"
      }
    ],
    "checkedAt": 1790536933728,
    "lastModified": 1790239043000,
    "etag": "W/\"def\""
  },
  "tama": {
    "models": [
      {
        "id": "Qwen/Qwen3.8-27B",
        "name": "Qwen3.8-27B",
        "reasoning": true,
        "thinkingLevelMap": { "off": "none", "minimal": null, "low": "low", "medium": "medium", "high": null, "xhigh": "xhigh", "max": null },
        "input": ["text", "image"],
        "contextWindow": 262144,
        "maxTokens": 32768,
        "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
        "compat": { "supportsReasoningEffort": true },
        "provider": "tama",
        "api": "openai-completions",
        "baseUrl": "https://tama.wizards.town/v1"
      }
    ],
    "checkedAt": 1790536933728
  }
}"#,
    );
    // The `pi-provider-*.json` fallback (a *different* schema — top-level
    // `baseURL` WITHOUT the `/v1` suffix) — the `tama/Unknown-Model`
    // entry (absent from `models-store.json`) must fall back to it.
    write_file(
        dir,
        "pi-provider-tama.json",
        r#"{
  "version": 1,
  "baseURL": "https://tama.wizards.town",
  "configHash": "abc",
  "lastFetchedMs": 1790536933728,
  "models": []
}"#,
    );
}

// ── (a) + (e) the full fixture → the expected `Model`s ───────────────

#[test]
fn seed_from_real_shapes_builds_the_expected_models() {
    let dir = tempfile::tempdir().unwrap();
    write_full_fixture(dir.path());

    // An empty `env_lookup` (the test is isolated from the real environment
    // — the production `seed_from` reads `std::env::var`, which a test cannot
    // control). The test asserts the `auth.json`-only `api_key` behavior.
    let catalog = ModelCatalog::seed_from_env(dir.path(), &|_| None);

    assert_eq!(catalog.all().len(), 4, "one Model per enabledModels entry");

    // `tama/Qwen/Qwen3.8-27B` — from the SPARSE `tama` provider entry
    // (only `{ models, checkedAt }`), split on the FIRST `/` (the model
    // id itself contains `/`).
    let tama = catalog
        .get("Qwen/Qwen3.8-27B")
        .expect("the default model is in the catalog");
    assert_eq!(tama.provider, "tama");
    assert_eq!(tama.base_url, "https://tama.wizards.town/v1");
    // `tama` is not in `auth.json` (a local gateway) → empty key.
    assert_eq!(tama.api_key, "");
    assert_eq!(tama.context_window, 262144);
    assert!(tama.cost_per_mtok_in.abs() < f64::EPSILON);
    assert!(tama.cost_per_mtok_out.abs() < f64::EPSILON);
    assert!(tama.supports_tools, "api == openai-completions");
    assert!(
        tama.supports_thinking,
        "reasoning + a non-null thinkingLevelMap"
    );
    let mut levels = tama.thinking_levels.clone();
    levels.sort();
    // The NON-NULL `thinkingLevelMap` keys (`minimal`/`high`/`max` are
    // null; `off` maps to `"none"` and counts).
    assert_eq!(levels, vec!["low", "medium", "off", "xhigh"]);

    // `openrouter/moonshotai/kimi-k3` — metadata + no auth entry
    // (empty key) + the model entry's `baseUrl` (with the `/v1` suffix).
    let kimi = catalog.get("moonshotai/kimi-k3").expect("kimi in catalog");
    assert_eq!(kimi.provider, "openrouter");
    assert_eq!(kimi.base_url, "https://openrouter.ai/api/v1");
    assert_eq!(kimi.api_key, "");
    assert_eq!(kimi.context_window, 1048576);
    assert!((kimi.cost_per_mtok_in - 0.8845).abs() < f64::EPSILON);
    assert!((kimi.cost_per_mtok_out - 10.5346).abs() < f64::EPSILON);
    assert!(kimi.supports_tools);
    assert!(kimi.supports_thinking);

    // `google/gemini-2.5-flash` — `api: "google-generative-ai"` is NOT
    // OpenAI-compatible → `supports_tools` false, and `auth.json`'s
    // `.key` is read (NOT `api_key`).
    let gemini = catalog.get("gemini-2.5-flash").expect("gemini in catalog");
    assert_eq!(gemini.provider, "google");
    assert_eq!(gemini.api_key, "gk_123");
    assert!(
        !gemini.supports_tools,
        "google-generative-ai is not OpenAI-compatible"
    );
    assert!(!gemini.supports_thinking, "no thinkingLevelMap");
    assert_eq!(gemini.thinking_levels, Vec::<String>::new());
    assert!((gemini.cost_per_mtok_in - 0.3).abs() < f64::EPSILON);

    // `tama/Unknown-Model` — no metadata in `models-store.json` → the
    // defaults (`context_window=128000`, `cost=0.0`,
    // `supports_tools=true`, `supports_thinking=false`) + the
    // `pi-provider-tama.json` fallback `baseURL` normalized (the
    // `/v1` suffix appended).
    let unknown = catalog.get("Unknown-Model").expect("unknown in catalog");
    assert_eq!(unknown.provider, "tama");
    assert_eq!(unknown.base_url, "https://tama.wizards.town/v1");
    assert_eq!(unknown.context_window, 128000);
    assert!(unknown.cost_per_mtok_in.abs() < f64::EPSILON);
    assert!(unknown.cost_per_mtok_out.abs() < f64::EPSILON);
    assert!(unknown.supports_tools);
    assert!(!unknown.supports_thinking);
}

#[test]
fn seed_from_seeds_the_default_and_compaction_from_settings() {
    let dir = tempfile::tempdir().unwrap();
    write_full_fixture(dir.path());

    let catalog = ModelCatalog::seed_from(dir.path());

    // (e) the default: the BARE `defaultModel` + `defaultProvider`
    // composed into the `"<provider>/<id>"` key, matched against the
    // `enabledModels` entries.
    assert_eq!(
        catalog.default_model.as_deref(),
        Some("tama/Qwen/Qwen3.8-27B")
    );
    // (e) the `Compactor` thresholds (Task 6) seeded from
    // `settings.json`'s `compaction` (NOT the hardcoded defaults).
    assert!(catalog.compaction.enabled);
    assert_eq!(catalog.compaction.reserve_tokens, 8192);
    assert_eq!(catalog.compaction.keep_recent_tokens, 40000);
}

// ── (b) a missing `auth.json` degrades (empty keys, not a crash) ─────

#[test]
fn seed_from_without_auth_file_yields_empty_api_keys() {
    let dir = tempfile::tempdir().unwrap();
    write_full_fixture(dir.path());
    std::fs::remove_file(dir.path().join("auth.json")).unwrap();

    // An empty `env_lookup` (isolated from the real environment — see the
    // `(a) + (e)` test above): the `api_key` comes from `auth.json` only.
    let catalog = ModelCatalog::seed_from_env(dir.path(), &|_| None);

    assert_eq!(
        catalog.all().len(),
        4,
        "the models survive without auth.json"
    );
    for m in catalog.all() {
        assert_eq!(
            m.api_key, "",
            "no auth.json → an empty key (degraded, not a crash)"
        );
    }
}

// ── (c) an unparseable `settings.json` degrades (empty catalog) ──────

#[test]
fn seed_from_with_unparseable_settings_yields_an_empty_catalog() {
    let dir = tempfile::tempdir().unwrap();
    write_file(dir.path(), "settings.json", "not json at all");
    write_file(
        dir.path(),
        "auth.json",
        r#"{ "google": { "type": "api_key", "key": "gk_123" } }"#,
    );
    write_file(dir.path(), "models-store.json", "{}");

    let catalog = ModelCatalog::seed_from(dir.path());

    assert!(catalog.all().is_empty(), "no enabledModels → no models");
    assert_eq!(catalog.default_model, None);
    // The compaction defaults (absent `compaction` key — compaction is
    // ON by default, pi's own behavior; an explicit
    // `compaction.enabled: false` disables it).
    assert!(catalog.compaction.enabled);
    assert_eq!(catalog.compaction.reserve_tokens, 16384);
    assert_eq!(catalog.compaction.keep_recent_tokens, 20000);
}

// ── a missing `pi_dir` degrades (empty catalog, not a crash) ─────────

#[test]
fn seed_from_missing_dir_yields_an_empty_catalog() {
    let dir = tempfile::tempdir().unwrap();
    let catalog = ModelCatalog::seed_from(&dir.path().join("does-not-exist"));
    assert!(catalog.all().is_empty());
    assert_eq!(catalog.default_model, None);
}

// ── a default matching no `enabledModels` entry → `None` ─────────────

#[test]
fn seed_from_default_matching_no_enabled_entry_is_none() {
    let dir = tempfile::tempdir().unwrap();
    write_file(
        dir.path(),
        "settings.json",
        r#"{
  "defaultProvider": "openai",
  "defaultModel": "gpt-x",
  "enabledModels": ["tama/Qwen/Qwen3.8-27B"]
}"#,
    );
    write_file(
        dir.path(),
        "models-store.json",
        r#"{
  "tama": {
    "models": [
      { "id": "Qwen/Qwen3.8-27B", "api": "openai-completions",
        "baseUrl": "https://tama.wizards.town/v1", "contextWindow": 262144,
        "cost": { "input": 0, "output": 0 } }
    ],
    "checkedAt": 1790536933728
  }
}"#,
    );

    let catalog = ModelCatalog::seed_from(dir.path());

    assert_eq!(catalog.all().len(), 1);
    assert_eq!(
        catalog.default_model, None,
        "the default matches no enabledModels entry"
    );
}

// ── (d) `openai_compatible()` filters to the v1-selectable set ───────

#[test]
fn openai_compatible_filters_to_the_openai_compatible_subset() {
    let dir = tempfile::tempdir().unwrap();
    write_full_fixture(dir.path());

    let catalog = ModelCatalog::seed_from(dir.path());

    let openai = catalog.openai_compatible();
    let ids: Vec<&str> = openai.iter().map(|m| m.id.as_str()).collect();
    // The two `api: "openai-completions"` models — NOT the
    // `google-generative-ai` model, NOT the metadata-less default model
    // (an unknown `api` is not OpenAI-compatible).
    assert_eq!(openai.len(), 2);
    assert!(ids.contains(&"Qwen/Qwen3.8-27B"));
    assert!(ids.contains(&"moonshotai/kimi-k3"));
    assert!(!ids.contains(&"gemini-2.5-flash"));
    assert!(!ids.contains(&"Unknown-Model"));
}
