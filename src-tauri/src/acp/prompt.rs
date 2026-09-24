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

/// 8 images per message — mirrors the frontend `MAX_CHAT_ATTACHMENTS` (ADR
/// 0008): re-validated here so hand-rolled IPC cannot bloat the DB
/// out-of-band (worst case 8 × 10 MiB raw ≈ 108 MB of base64 persisted per
/// message — base64 expands each image ~4/3×).
pub const MAX_IMAGE_COUNT: usize = 8;

/// 255 bytes — the OS per-component filename limit: `name` is written verbatim
/// to SQLite, so re-validated here so hand-rolled IPC cannot bloat a row
/// out-of-band. (The frontend's `name` comes from a real OS filename and is
/// already ≤255 bytes on all major OSes.)
pub const MAX_IMAGE_NAME_LEN: usize = 255;

/// Deliberately NARROWER than `image/*`: the destination is LLM vision APIs
/// (png/jpeg/gif/webp only — an SVG would fail the whole turn at the
/// provider). Mirrors the frontend `SUPPORTED_IMAGE_TYPES`.
const SUPPORTED_IMAGE_TYPES: &[&str] = &["image/png", "image/jpeg", "image/gif", "image/webp"];

/// Build the prompt's content blocks: the text block FIRST, then one image
/// block per attachment (matches pi's own user-content ordering).
///
/// Validation (guards against hand-rolled IPC):
/// - `mime_type` must be in `SUPPORTED_IMAGE_TYPES`;
/// - `name` must be ≤ `MAX_IMAGE_NAME_LEN` bytes (it is written verbatim to
///   SQLite — an unbounded `name` could bloat a row out-of-band);
/// - `data` must have ≤2 trailing `=` padding chars (legitimate base64 has
///   0–2; more would bypass the size cap, since the estimate strips them all
///   when measuring and the full string is persisted);
/// - decoded size must be ≤ `MAX_IMAGE_BYTES`. The estimate strips trailing
///   `=` padding first, then takes `len * 3 / 4` (exact for well-formed
///   base64; a safe over-estimate otherwise; `size_bytes` is NOT trusted).
///   Stripping the padding keeps the bound consistent with the frontend's
///   `file.size <= 10 MiB` cap (no off-by-2 rejections).
/// - the image count must be ≤ `MAX_IMAGE_COUNT`.
pub fn build_prompt_blocks(
    text: &str,
    images: &[ImagePayload],
) -> Result<Vec<ContentBlock>, AcpError> {
    if images.len() > MAX_IMAGE_COUNT {
        return Err(AcpError::InvalidPrompt {
            message: format!("at most {MAX_IMAGE_COUNT} images per message"),
        });
    }
    // Skip the text block when it would be EMPTY and images are present
    // (the frontend supports image-only sends; strict providers reject an
    // empty text block). Keep the text block when there are no images, so a
    // text-only call is byte-identical to before.
    let mut blocks = Vec::new();
    if !text.is_empty() || images.is_empty() {
        blocks.push(ContentBlock::Text(TextContent::new(text)));
    }
    for img in images {
        if img.name.len() > MAX_IMAGE_NAME_LEN {
            return Err(AcpError::InvalidPrompt {
                message: "image name exceeds 255 bytes".to_string(),
            });
        }
        if !SUPPORTED_IMAGE_TYPES.contains(&img.mime_type.as_str()) {
            return Err(AcpError::InvalidPrompt {
                message: format!(
                    "unsupported image type: {} (expected png, jpeg, gif, webp)",
                    img.mime_type
                ),
            });
        }
        let trimmed = img.data.trim_end_matches('=');
        // Legitimate base64 has 0–2 trailing `=` padding. Rejecting >2 keeps
        // the stripped-size estimate a real bound on the persisted data
        // (arbitrary `=` padding would otherwise bypass the size cap).
        let padding = img.data.len() - trimmed.len();
        if padding > 2 {
            return Err(AcpError::InvalidPrompt {
                message: "malformed base64 image data".to_string(),
            });
        }
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
    fn exactly_max_size_image_is_accepted() {
        // The boundary: the padding-stripped estimate is EXACTLY 10 MiB
        // (13_981_014 * 3 / 4 = 10_485_760) — accepted, not rejected. This
        // is the whole reason the `=` padding is stripped: a refactor to
        // un-stripped `len * 3 / 4` or `>=` would silently regress the
        // boundary (the frontend already accepted this file).
        let data = "A".repeat(13_981_014) + "==";
        let im = ImagePayload {
            mime_type: "image/png".into(),
            data,
            name: "max.png".into(),
            size_bytes: MAX_IMAGE_BYTES,
        };
        assert!(
            build_prompt_blocks("x", &[im]).is_ok(),
            "exactly 10 MiB (the cap) must be accepted, not rejected"
        );
    }

    #[test]
    fn excessive_padding_is_rejected() {
        // Legitimate base64 has 0–2 trailing `=` padding. A hand-rolled IPC
        // payload can append arbitrarily many `=` to small data: the size
        // estimate strips them ALL when measuring, so without this check the
        // 10 MiB cap would not bound the persisted data. 5 padding chars on
        // small data (decoded estimate well under the cap) must be rejected.
        let data = "AQID".repeat(25) + "====="; // 5 padding chars, small data
        let im = ImagePayload {
            mime_type: "image/png".into(),
            data,
            name: "p.png".into(),
            size_bytes: 25,
        };
        assert!(matches!(
            build_prompt_blocks("x", &[im]),
            Err(AcpError::InvalidPrompt { .. })
        ));
    }

    #[test]
    fn name_over_255_bytes_is_rejected() {
        // `name` is written verbatim to SQLite: an unbounded `name` from
        // hand-rolled IPC could bloat a row. 256 bytes must be rejected.
        let im = ImagePayload {
            mime_type: "image/png".into(),
            data: "AQID".into(),
            name: "a".repeat(256),
            size_bytes: 3,
        };
        assert!(matches!(
            build_prompt_blocks("x", &[im]),
            Err(AcpError::InvalidPrompt { .. })
        ));
    }

    #[test]
    fn name_exactly_255_bytes_is_accepted() {
        // The boundary: exactly 255 bytes is the OS per-component filename
        // limit and must be accepted, not rejected.
        let im = ImagePayload {
            mime_type: "image/png".into(),
            data: "AQID".into(),
            name: "a".repeat(255),
            size_bytes: 3,
        };
        assert!(build_prompt_blocks("x", &[im]).is_ok());
    }

    #[test]
    fn too_many_images_are_rejected() {
        // Mirrors the frontend `MAX_CHAT_ATTACHMENTS` (ADR 0008): a 9th valid
        // image from hand-rolled IPC must be rejected, not just the oversized
        // or wrong-mime cases.
        let images: Vec<ImagePayload> = (0..9)
            .map(|i| img("image/png", 4, &format!("a{i}.png")))
            .collect();
        assert!(matches!(
            build_prompt_blocks("x", &images),
            Err(AcpError::InvalidPrompt { .. })
        ));
    }

    #[test]
    fn empty_text_with_images_ships_no_text_block() {
        // The frontend supports image-only sends (empty draft + staged
        // images): a strict provider (e.g. Anthropic) rejects an empty text
        // block, so the client must not ship one. Exactly one block (the
        // image), no text block.
        let im = img("image/png", 4, "a.png");
        let blocks = build_prompt_blocks("", &[im]).expect("an image-only prompt should be valid");
        assert_eq!(
            blocks.len(),
            1,
            "exactly one block (the image; no empty text block)"
        );
        let agent_client_protocol::schema::v1::ContentBlock::Image(i) = &blocks[0] else {
            panic!("expected the single block to be the image block");
        };
        assert_eq!(i.mime_type, "image/png");
        assert_eq!(i.data, "AAAA");
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
