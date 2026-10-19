//! `--qr`: a block as a QR code drawn with block characters, for a terminal
//! with no clipboard. Only the lines Kyumei reads go into the code; the
//! readable lines stay on screen above it.

use qrcode::{EcLevel, QrCode, render::unicode::Dense1x2};

/// The lines of a block that Kyumei reads: its first and last lines and the
/// payload.
pub fn essential(block: &str) -> String {
    block
        .lines()
        .filter(|l| l.starts_with("-----") || l.starts_with(':'))
        .map(|l| format!("{l}\n"))
        .collect()
}

/// Draw `text` as a QR code, light modules dark, for a dark terminal.
pub fn render(text: &str) -> Result<String, String> {
    let code = QrCode::with_error_correction_level(text.as_bytes(), EcLevel::L).map_err(|_| {
        format!(
            "the block is too large for one QR code ({} bytes; at most 2,953). Narrow the window, raise the step, or copy the text instead",
            text.len()
        )
    })?;
    Ok(code
        .render::<Dense1x2>()
        .dark_color(Dense1x2::Light)
        .light_color(Dense1x2::Dark)
        .quiet_zone(true)
        .build())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_blocks_draw_and_large_ones_say_why_not() {
        let block = "-----BEGIN KYUMEI SAMPLES v1-----\nCollected by x\n:AAAA\n-----END KYUMEI SAMPLES (4 chars, crc32 00000000)-----\n";
        assert_eq!(
            essential(block),
            "-----BEGIN KYUMEI SAMPLES v1-----\n:AAAA\n-----END KYUMEI SAMPLES (4 chars, crc32 00000000)-----\n"
        );
        assert!(render(&essential(block)).is_ok());
        let err = render(&"x".repeat(4000)).expect_err("too large");
        assert!(err.contains("too large"), "{err}");
    }
}
