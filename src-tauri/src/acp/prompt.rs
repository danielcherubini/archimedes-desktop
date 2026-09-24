//! Pure helpers for building prompt requests and persisting user messages.
//! Shared by the `send_prompt` Tauri command and `SessionManager::send_prompt`
//! (the two paths are separate — see `commands/sessions.rs`).

use agent_client_protocol::schema::v1::{ContentBlock, ImageContent, TextContent};

use crate::acp::AcpError;

/// One image attachment over IPC. The frontend sends CAMEL CASE
/// (`mimeType`, `sizeBytes`) — Tauri camel-cases only the TOP-LEVEL command
/// args, so this nested struct renames explicitly.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImagePayload {
    pub mime_type: String,
    /// base64, WITHOUT a `data:` prefix.
    pub data: String,
    pub name: String,
    pub size_bytes: u64,
}

/// 10 MiB — mirrors the frontend cap (ADR 0008).
pub const MAX_IMAGE_BYTES: u64 = 10 * 1024 * 1024;

/// Deliberately NARROWER than `image/*`: the destination is LLM vision APIs
/// (png/jpeg/gif/webp only — an SVG would fail the whole turn at the
/// provider). Mirrors the frontend `SUPPORTED_IMAGE_TYPES`.
const SUPPORTED_IMAGE_TYPES: &[&str] = &["image/png", "image/jpeg", "image/gif", "image/webp"];

/// Build the prompt's content blocks: the text block FIRST, then one image
/// block per attachment (matches pi's own user-content ordering).
///
/// Validation (guards against hand-rolled IPC):
/// - `mime_type` must be in `SUPPORTED_IMAGE_TYPES`;
/// - decoded size must be ≤ `MAX_IMAGE_BYTES`. The estimate strips trailing
///   `=` padding first, then takes `len * 3 / 4` (an upper bound; `size_bytes`
///   is NOT trusted). Stripping the padding keeps the bound consistent with
///   the frontend's `file.size <= 10 MiB` cap (no off-by-2 rejections).
pub fn build_prompt_blocks(
    text: &str,
    images: &[ImagePayload],
) -> Result<Vec<ContentBlock>, AcpError> {
    let mut blocks = vec![ContentBlock::Text(TextContent::new(text))];
    for img in images {
        if !SUPPORTED_IMAGE_TYPES.contains(&img.mime_type.as_str()) {
            return Err(AcpError::InvalidPrompt {
                message: format!(
                    "unsupported image type: {} (expected png, jpeg, gif, webp)",
                    img.mime_type
                ),
            });
        }
        let trimmed = img.data.trim_end_matches('=');
        let decoded_bytes = (trimmed.len() as u64) * 3 / 4;
        if decoded_bytes > MAX_IMAGE_BYTES {
            return Err(AcpError::InvalidPrompt {
                message: "image exceeds the 10 MiB limit".to_string(),
            });
        }
        blocks.push(ContentBlock::Image(ImageContent::new(
            img.data.clone(),
            img.mime_type.clone(),
        )));
    }
    Ok(blocks)
}

/// The user-message payload persisted in `messages.payload_json` (ADR 0008):
/// `{ "text": ..., "images": [{ name, mimeType, sizeBytes, data }] }` —
/// the `images` key is OMITTED when empty (pre-feature rows stay `{"text"}`).
pub fn user_message_payload(text: &str, images: &[ImagePayload]) -> serde_json::Value {
    if images.is_empty() {
        serde_json::json!({ "text": text })
    } else {
        serde_json::json!({
            "text": text,
            "images": images.iter().map(|img| {
                serde_json::json!({
                    "name": img.name,
                    "mimeType": img.mime_type,
                    "sizeBytes": img.size_bytes,
                    "data": img.data,
                })
            }).collect::<Vec<_>>(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::SessionInfo;
    use crate::storage::Db;
    use agent_client_protocol::schema::v1::AgentCapabilities;

    fn img(mime: &str, data_len: usize, name: &str) -> ImagePayload {
        ImagePayload {
            mime_type: mime.to_string(),
            data: "A".repeat(data_len),
            name: name.to_string(),
            size_bytes: (data_len as u64) * 3 / 4,
        }
    }

    /// `temp_config_dir` lives in `session.rs`'s private test module and is
    /// not importable here — inline the equivalent (a unique temp dir).
    fn fresh_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir(&dir).unwrap();
        dir
    }

    #[test]
    fn zero_images_is_a_single_text_block() {
        let blocks = build_prompt_blocks("hi", &[]).expect("text-only should be valid");
        assert_eq!(blocks.len(), 1);
        let agent_client_protocol::schema::v1::ContentBlock::Text(t) = &blocks[0] else {
            panic!("expected a text block");
        };
        assert_eq!(t.text, "hi");
    }

    #[test]
    fn text_block_first_then_images_in_order() {
        let images = vec![
            ImagePayload {
                mime_type: "image/png".into(),
                data: "AAA".into(),
                name: "a.png".into(),
                size_bytes: 2,
            },
            ImagePayload {
                mime_type: "image/jpeg".into(),
                data: "BBB".into(),
                name: "b.jpg".into(),
                size_bytes: 2,
            },
        ];
        let blocks = build_prompt_blocks("hi", &images).expect("valid images should pass");
        assert_eq!(blocks.len(), 3);
        let agent_client_protocol::schema::v1::ContentBlock::Text(t) = &blocks[0] else {
            panic!("expected a text block first");
        };
        assert_eq!(t.text, "hi");
        let agent_client_protocol::schema::v1::ContentBlock::Image(i) = &blocks[1] else {
            panic!("expected an image block");
        };
        assert_eq!(i.mime_type, "image/png");
        assert_eq!(i.data, "AAA");
        let agent_client_protocol::schema::v1::ContentBlock::Image(i) = &blocks[2] else {
            panic!("expected an image block");
        };
        assert_eq!(i.mime_type, "image/jpeg");
        assert_eq!(i.data, "BBB");
    }

    #[test]
    fn non_image_mime_is_rejected() {
        let im = img("text/plain", 10, "x");
        assert!(matches!(
            build_prompt_blocks("x", &[im]),
            Err(AcpError::InvalidPrompt { .. })
        ));
    }

    #[test]
    fn svg_is_rejected() {
        // The allowlist, not a prefix check: `image/svg+xml` starts with
        // `image/` but is NOT in `SUPPORTED_IMAGE_TYPES`.
        let im = img("image/svg+xml", 10, "x.svg");
        assert!(matches!(
            build_prompt_blocks("x", &[im]),
            Err(AcpError::InvalidPrompt { .. })
        ));
    }

    #[test]
    fn oversized_image_is_rejected() {
        // Decoded estimate: 14_000_000 * 3 / 4 = 10_500_000 > 10 MiB (10_485_760).
        let im = img("image/png", 14_000_000, "big");
        assert!(matches!(
            build_prompt_blocks("x", &[im]),
            Err(AcpError::InvalidPrompt { .. })
        ));
    }

    #[test]
    fn payload_omits_images_key_when_empty() {
        let v = user_message_payload("hi", &[]);
        assert_eq!(v["text"], "hi");
        assert!(v.get("images").is_none());
    }

    #[test]
    fn payload_includes_images_with_camel_case_keys() {
        let im = img("image/png", 4, "a.png"); // data "AAAA", size_bytes 3
        let v = user_message_payload("hi", &[im]);
        assert_eq!(v["text"], "hi");
        let img0 = &v["images"][0];
        assert_eq!(img0["name"], "a.png");
        assert_eq!(img0["mimeType"], "image/png");
        assert_eq!(img0["sizeBytes"], 3);
        assert_eq!(img0["data"], "AAAA");
    }

    #[test]
    fn db_round_trip_preserves_images() {
        let dir = fresh_dir();
        let db = Db::open(&dir.join("archimedes.db")).expect("db should open");
        // FK: `messages.session_id REFERENCES sessions(id)` and `Db::open`
        // enables `PRAGMA foreign_keys` — record the session FIRST, or
        // `record_message` fails.
        db.record_session(&SessionInfo {
            session_id: "x".into(),
            agent_id: "fake".into(),
            cwd: "/tmp/x".into(),
            capabilities: AgentCapabilities::default(),
            config_options: None,
        })
        .expect("record_session should succeed");
        let im = img("image/png", 4, "a.png");
        db.record_message(
            "x",
            "user",
            None,
            &user_message_payload("hi", &[im]).to_string(),
        )
        .expect("record_message should succeed");
        let rows = db.messages_for("x").expect("messages_for should succeed");
        assert_eq!(rows.len(), 1);
        let payload: serde_json::Value =
            serde_json::from_str(&rows[0].payload_json).expect("payload_json should parse");
        assert_eq!(payload["text"], "hi");
        let arr = payload["images"]
            .as_array()
            .expect("images should round-trip");
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["name"], "a.png");
        assert_eq!(arr[0]["mimeType"], "image/png");
        assert_eq!(arr[0]["sizeBytes"], 3);
        assert_eq!(arr[0]["data"], "AAAA");
    }
}
