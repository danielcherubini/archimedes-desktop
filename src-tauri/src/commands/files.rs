//! File-reading commands (the composer's `+` file picker).
//!
//! `read_file_bytes` is the backend half of the image picker: the frontend
//! opens the native file dialog (`tauri-plugin-dialog` — a selection is a
//! PATH, not a `File`), then reads the bytes here. The webview cannot read
//! an arbitrary local path itself (no `fs` plugin), so the read is a
//! Tauri command.

use crate::agent::MAX_IMAGE_BYTES;

/// The image extensions the picker accepts — the SAME allowlist as the
/// frontend's `SUPPORTED_IMAGE_TYPES` (the MIMEs the vision APIs accept:
/// an SVG passes an `image/*` filter but fails at the provider, so the
/// allowlist is enforced here AND in the frontend).
pub const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp"];

/// Is the path's extension in the image allowlist (case-insensitive; a
/// path with no extension is `false`)?
pub fn is_supported_image_path(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| IMAGE_EXTENSIONS.contains(&e.to_lowercase().as_str()))
}

/// Is a file of `len` bytes within the prompt path's 10 MiB cap
/// (`MAX_IMAGE_BYTES`, ADR 0008)? Factored out as a pure comparison so
/// the check is unit-testable without writing a 10 MiB file (the same
/// pattern as the clipboard path's `encoded_png_within_cap`).
pub fn image_within_cap(len: u64) -> bool {
    len <= MAX_IMAGE_BYTES
}

/// Read a picked file's bytes (an image for the composer's attachments).
///
/// TWO-TIER BOUND (defense in depth — the dialog already filters by these
/// extensions, but the command is public IPC): the extension must be in
/// the image allowlist (a miss is `Ok(None)` — "not an image", the
/// frontend skips it silently) AND the size must fit `MAX_IMAGE_BYTES`
/// (an over-cap file is an `Err` — the frontend shows it on the
/// composer's error line). The read is blocking (a disk I/O), so it runs
/// on a worker thread.
#[tauri::command]
pub async fn read_file_bytes(path: String) -> Result<Option<Vec<u8>>, String> {
    if !is_supported_image_path(&path) {
        return Ok(None);
    }
    tauri::async_runtime::spawn_blocking(move || {
        let meta = std::fs::metadata(&path).map_err(|e| e.to_string())?;
        if !image_within_cap(meta.len()) {
            return Err(format!(
                "the file is {} bytes, which exceeds the 10 MiB limit",
                meta.len()
            ));
        }
        std::fs::read(&path).map(Some).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use crate::agent::MAX_IMAGE_BYTES;
    use crate::commands::files::{image_within_cap, is_supported_image_path, read_file_bytes};

    #[test]
    fn is_supported_image_path_accepts_the_allowlist_case_insensitively() {
        for ext in ["png", "jpg", "jpeg", "gif", "webp", "PNG", "Jpg"] {
            assert!(
                is_supported_image_path(&format!("/home/u/pics/a.{ext}")),
                "{ext} should be accepted"
            );
        }
    }

    #[test]
    fn is_supported_image_path_rejects_non_images_and_extensionless_paths() {
        for path in [
            "/home/u/docs/notes.txt",
            "/home/u/docs/diagram.svg",
            "/home/u/pics/noext",
            "/home/u/pics",
        ] {
            assert!(!is_supported_image_path(path), "{path} should be rejected");
        }
    }

    #[test]
    fn image_within_cap_bounded_by_max_image_bytes() {
        assert!(image_within_cap(0));
        assert!(image_within_cap(MAX_IMAGE_BYTES));
        assert!(!image_within_cap(MAX_IMAGE_BYTES + 1));
    }

    #[tokio::test]
    async fn read_file_bytes_returns_the_file_content_for_a_picked_image() {
        let dir = std::env::temp_dir().join(format!("files-cmd-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("shot.png");
        let bytes: Vec<u8> = vec![0x89, 0x50, 0x4E, 0x47, 1, 2, 3];
        std::fs::write(&path, &bytes).unwrap();
        let result = read_file_bytes(path.to_string_lossy().into()).await;
        assert_eq!(result, Ok(Some(bytes)));
    }

    #[tokio::test]
    async fn read_file_bytes_returns_none_for_a_non_image_extension() {
        let dir = std::env::temp_dir().join(format!("files-cmd-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("notes.txt");
        std::fs::write(&path, b"hello").unwrap();
        let result = read_file_bytes(path.to_string_lossy().into()).await;
        assert_eq!(result, Ok(None));
    }

    #[tokio::test]
    async fn read_file_bytes_errors_when_the_file_cannot_be_read() {
        // A supported extension but a missing file: the read fails (the
        // dialog selected a path that is gone by the time we read it —
        // e.g. the user deleted it).
        let result = read_file_bytes("/no/such/dir/shot.png".to_string()).await;
        let err = result.expect_err("a missing file is an error");
        assert!(!err.is_empty());
    }
}
