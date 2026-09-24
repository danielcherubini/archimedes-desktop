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

/// Re-encodes raw RGBA bytes (length `width * height * 4`) as a PNG buffer.
///
/// Pure (no clipboard access) so the encoding is unit-testable; the
/// clipboard read itself needs a live display.
///
/// BOUNDS THE ALLOCATION (security): `arboard::get_image` has ALREADY
/// decoded the clipboard image to RGBA (`width * height * 4` bytes — a
/// 10000 x 10000 image is ~400 MB) INSIDE arboard, which we cannot cap
/// without forking it. This check (against the prompt path's `MAX_IMAGE_BYTES`
/// 10 MiB cap, ADR 0008) bounds what WE control: the second allocation
/// (the `rgba.to_vec()` copy + the PNG encode buffer) and the IPC transfer
/// of a huge payload past the frontend's 10 MiB cap. An oversized payload
/// is rejected (no copy, no encode, no IPC transfer).
pub fn rgba_to_png_bytes(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    // `u64` math: `width * height * 4` overflows `u32` for realistic sizes
    // (a 30000 x 30000 image is ~3.6 GB).
    let raw_bytes = u64::from(width) * u64::from(height) * 4;
    if raw_bytes > MAX_IMAGE_BYTES {
        return Err(format!(
            "clipboard image is {raw_bytes} bytes ({width}x{height} RGBA), which exceeds the 10 MiB limit"
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
    fn oversized_image_is_rejected_at_the_cap() {
        // 5120 x 513 RGBA = 10_506_240 bytes, just over the 10 MiB cap
        // (10_485_760): the payload is rejected (no second allocation, no
        // PNG encode, no IPC transfer of a huge payload). A 10000 x
        // 10000 image (~400 MB) would be rejected by the same check.
        let rgba = vec![0u8; 5120 * 513 * 4];
        let result = rgba_to_png_bytes(5120, 513, &rgba);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("exceeds the 10 MiB"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn exactly_cap_sized_image_is_accepted() {
        // The boundary: 5120 x 512 RGBA = exactly 10_485_760 bytes = the
        // cap — accepted, not rejected (`>` not `>=`).
        let rgba = vec![0u8; 5120 * 512 * 4];
        assert!(rgba_to_png_bytes(5120, 512, &rgba).is_ok());
    }
}
