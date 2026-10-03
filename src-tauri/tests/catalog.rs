//! `ModelCatalog` (native-agent-harness Task 5, ADR 0014): the desktop is
//! native-only — the catalog is the Settings' providers list + live
//! discovery (no pi config seeding; that path was removed, ADR 0022).

use archimedes_lib::agent::harness::{CompactionConfig, Model, ModelCatalog};

fn model(id: &str, provider: &str, api: Option<&str>) -> Model {
    Model {
        id: id.to_string(),
        provider: provider.to_string(),
        base_url: format!("http://fake-{provider}/v1"),
        api_key: "k".to_string(),
        context_window: 100_000,
        cost_per_mtok_in: 0.0,
        cost_per_mtok_out: 0.0,
        supports_tools: true,
        supports_thinking: false,
        thinking_levels: Vec::new(),
        api: api.map(|s| s.to_string()),
    }
}

#[test]
fn selectable_filters_to_the_three_wires_subset() {
    let catalog = ModelCatalog {
        models: vec![
            model("Qwen/Qwen3.8-27B", "tama", Some("openai-completions")),
            model(
                "moonshotai/kimi-k3",
                "openrouter",
                Some("openai-completions"),
            ),
            model("gemini-2.5-flash", "google", Some("google-generative-ai")),
            model("Unknown-Model", "tama", None),
        ],
        default_model: Some("Qwen/Qwen3.8-27B".to_string()),
        compaction: CompactionConfig::default(),
    };

    let selectable = catalog.selectable();
    let ids: Vec<&str> = selectable.iter().map(|m| m.id.as_str()).collect();
    // The two `api: "openai-completions"` models — NOT the
    // `google-generative-ai` model, NOT the metadata-less default model
    // (an unknown `api` is not one of the three the harness speaks).
    assert_eq!(selectable.len(), 2);
    assert!(ids.contains(&"Qwen/Qwen3.8-27B"));
    assert!(ids.contains(&"moonshotai/kimi-k3"));
    assert!(!ids.contains(&"gemini-2.5-flash"));
    assert!(!ids.contains(&"Unknown-Model"));
}
