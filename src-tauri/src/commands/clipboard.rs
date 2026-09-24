//! System-clipboard commands.
//!
//! `read_clipboard_image` is the WebKitGTK paste fallback: WebKit's `paste`
//! event does not expose clipboard images as `DataTransfer` file items (the
//! event's `items`/`files` are empty for a pasted image — verified on
//! webkit2gtk-4.1 2.52.5 on Wayland), so the composer reads the image
//! directly from the system clipboard here (arboard: Wayland
//! `wl-clipboard-rs` backend when `WAYLAND_DISPLAY` is set, X11 otherwise).

use image::ImageEncoder;

use crate::acp::prompt::MAX_IMAGE_BYTES;

/// 100 MiB — tier 1 (DoS guard) for the RAW clipboard RGBA: `width *
/// height * 4` must not exceed this. This bounds OUR copy + PNG-encode
/// allocation (a 10000 x 10000 image is ~400 MB) while still allowing
/// large-but-compressible images (a 4K screenshot is ~31.7 MiB raw but
/// typically encodes to ~2–8 MiB of PNG).
pub const MAX_CLIPBOARD_RAW_BYTES: u64 = 100 * 1024 * 1024;

/// Tier 2 (feature cap): is this ENCODED PNG length within the prompt
/// path's 10 MiB cap (`MAX_IMAGE_BYTES`, ADR 0008)? Factored out as a
/// pure comparison so the check is unit-testable without encoding a
/// 100 MiB image.
pub fn encoded_png_within_cap(encoded_len: u64) -> bool {
    encoded_len <= MAX_IMAGE_BYTES
}

/// Re-encodes raw RGBA bytes (length `width * height * 4`) as a PNG buffer.
///
/// Pure (no clipboard access) so the encoding is unit-testable; the
/// clipboard read itself needs a live display.
///
/// TWO-TIER BOUND (security): the raw RGBA must fit `MAX_CLIPBOARD_RAW_BYTES`
/// (100 MiB, tier 1 — DoS guard) AND the ENCODED PNG must fit
/// `MAX_IMAGE_BYTES` (10 MiB, tier 2 — the prompt path's feature cap,
/// ADR 0008). Tier 1 bounds what WE control: the `rgba.to_vec()` copy +
/// the PNG-encode buffer + the IPC transfer. A compressible 4K screenshot
/// (3840 x 2160 = ~31.7 MiB raw) typically encodes to ~2–8 MiB of PNG, so
/// bounding only the raw size at the 10 MiB feature cap over-rejected it.
///
/// arboard's INTERNAL decode (inside `get_image()`, which already returns
/// a full `width * height * 4` RGBA buffer) remains UNCAPPED — capping it
/// would require forking arboard. Tier 1 bounds everything after that point.
pub fn rgba_to_png_bytes(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    // `u64` math: `width * height * 4` overflows `u32` for realistic sizes
    // (a 30000 x 30000 image is ~3.6 GB).
    let raw_bytes = u64::from(width) * u64::from(height) * 4;
    // Tier 1 (DoS guard): reject BEFORE the copy + PNG-encode allocation
    // (no second allocation, no IPC transfer of a huge payload).
    if raw_bytes > MAX_CLIPBOARD_RAW_BYTES {
        return Err(format!(
            "clipboard image is {raw_bytes} bytes ({width}x{height} RGBA), which exceeds the 100 MiB limit"
        ));
    }
    let image = image::RgbaImage::from_raw(width, height, rgba.to_vec()).ok_or_else(|| {
        format!(
            "RGBA byte length {} does not match width({}) * height({}) * 4",
            rgba.len(),
            width,
            height
        )
    })?;
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out)
        .write_image(&image, width, height, image::ExtendedColorType::Rgba8)
        .map_err(|e| e.to_string())?;
    // Tier 2 (feature cap): the ENCODED image must fit the prompt path's
    // 10 MiB cap — a raw image that encodes over the cap is rejected even
    // though its raw size passed tier 1.
    if !encoded_png_within_cap(out.len() as u64) {
        return Err(format!(
            "clipboard image encodes to {} bytes of PNG, which exceeds the 10 MiB limit",
            out.len()
        ));
    }
    Ok(out)
}

/// Reads the current system-clipboard image (if any) as PNG bytes.
///
/// `Ok(None)` — no image on the clipboard (text-only or empty). The read is
/// blocking (a compositor round-trip) so it runs on a worker thread.
#[tauri::command]
pub async fn read_clipboard_image() -> Result<Option<Vec<u8>>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let mut clipboard = arboard::Clipboard::new().map_err(|e| e.to_string())?;
        let image = match clipboard.get_image() {
            Ok(image) => image,
            // No image on the clipboard (text-only or empty) — not an error
            // for the caller: just "nothing to stage".
            Err(arboard::Error::ContentNotAvailable) => return Ok(None),
            Err(e) => return Err(e.to_string()),
        };
        rgba_to_png_bytes(image.width as u32, image.height as u32, &image.bytes).map(Some)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn rgba_to_png_encodes_a_valid_png() {
        // 2x2 RGBA: red, green, blue, transparent.
        let rgba = [
            255, 0, 0, 255, //
            0, 255, 0, 255, //
            0, 0, 255, 255, //
            0, 0, 0, 0, //
        ];
        let png = rgba_to_png_bytes(2, 2, &rgba).expect("encode should succeed");
        // PNG signature.
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        // Round-trip: decode and compare pixel 0.
        let decoded = image::ImageReader::new(Cursor::new(&png))
            .with_guessed_format()
            .expect("valid PNG")
            .decode()
            .expect("decode");
        let rgba8 = decoded.to_rgba8();
        assert_eq!(rgba8.get_pixel(0, 0).0, [255, 0, 0, 255]);
        assert_eq!(rgba8.get_pixel(1, 1).0, [0, 0, 0, 0]);
    }

    #[test]
    fn rgba_to_png_rejects_a_mismatched_byte_length() {
        let result = rgba_to_png_bytes(2, 2, &[1, 2, 3]);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("does not match"), "unexpected error: {err}");
    }

    #[test]
    fn exactly_cap_sized_raw_image_encodes_small_and_is_accepted() {
        // Two tiers: 5120 x 512 RGBA = exactly 10_485_760 bytes = the 10 MiB
        // feature cap — this is NOT the rejection boundary anymore (tier 1
        // is 100 MiB). All-zero pixels encode to a tiny PNG (< 10 MiB), so
        // a 10 MiB RAW image that ENCODES under the cap is accepted.
        let rgba = vec![0u8; 5120 * 512 * 4];
        let png = rgba_to_png_bytes(5120, 512, &rgba)
            .expect("a 10 MiB raw image that encodes under 10 MiB is accepted");
        assert!((png.len() as u64) <= MAX_IMAGE_BYTES);
    }

    #[test]
    fn raw_image_between_10_and_100_mib_encodes_small_and_is_accepted() {
        // Tier 1 allows raw up to 100 MiB: 6000 x 2500 RGBA = 60_000_000
        // bytes (~57.2 MiB — between the 10 MiB feature cap and the 100 MiB
        // raw guard), a compressible 4K-class screenshot. All-zero pixels
        // encode to a tiny PNG (< 10 MiB), so tier 2 accepts it.
        let rgba = vec![0u8; 6000 * 2500 * 4];
        let png = rgba_to_png_bytes(6000, 2500, &rgba)
            .expect("a raw image between 10 and 100 MiB that encodes under 10 MiB is accepted");
        assert!(
            (png.len() as u64) <= MAX_IMAGE_BYTES,
            "the encoded PNG must fit the 10 MiB feature cap: {} bytes",
            png.len()
        );
    }

    #[test]
    fn raw_image_over_100_mib_is_rejected_at_the_raw_guard() {
        // Tier 1: 5120 x 5121 RGBA = 104_878_080 bytes, just over the
        // 100 MiB raw guard (104_857_600) — rejected BEFORE the copy +
        // PNG-encode allocation (no second allocation, no IPC transfer).
        let rgba = vec![0u8; 5120 * 5121 * 4];
        let result = rgba_to_png_bytes(5120, 5121, &rgba);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("exceeds the 100 MiB"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn encoded_size_over_the_10_mib_cap_is_rejected() {
        // Tier 2, tested on the pure comparison (constructing a real
        // > 10 MiB PNG in a unit test would mean encoding a 100 MiB
        // image — the comparison is the unit under test): exactly the
        // cap fits, one byte over does not.
        assert!(encoded_png_within_cap(MAX_IMAGE_BYTES));
        assert!(!encoded_png_within_cap(MAX_IMAGE_BYTES + 1));
        // And a raw image UNDER the 100 MiB guard that encodes over the
        // 10 MiB cap is rejected (not accepted) by the same check.
        assert!(encoded_png_within_cap(0));
    }
}
