//! Receipt bitmap → ESC/POS raster bit image.
//!
//! A rasterised receipt goes to the printer as dots at its native pitch, so no
//! Windows form, driver page length, or Chromium pagination can shrink it or push
//! the last row onto a second page. That is why HTML receipts are rendered to a
//! bitmap instead of being handed to the printer driver.

/// Rows per `GS v 0` band. Small printers have a few KB of buffer, so long
/// receipts are streamed in bands instead of one huge bit image.
const BAND_ROWS: u32 = 128;

/// ~1.5 m of paper. Anything longer is a runaway receipt, not a real sale.
pub const MAX_RASTER_ROWS: u32 = 12_000;

/// Luma below this burns a dot. Receipts are text and rules, so a hard threshold
/// stays crisp where dithering would turn small glyphs into grey mush.
pub const BLACK_THRESHOLD: u8 = 176;

/// `(layout width in mm, head width in dots)` for a roll size.
///
/// The layout width is the *printable* width, not the roll width: a 203 dpi head
/// burns 8 dots/mm, so 384 dots covers 48mm of a 58mm roll and 576 dots covers
/// 72mm of an 80mm roll, centred with unprintable paper either side.
///
/// Laying out at the roll width instead would squeeze the whole receipt to ~90%,
/// which is what made printed text smaller than the design intends.
pub fn paper_geometry(paper_mm: u32) -> (u32, u32) {
    if paper_mm <= 58 {
        (48, 384)
    } else {
        (72, 576)
    }
}

/// Device scale that makes the laid-out width land on exactly `width_dots`, so one
/// rendered pixel becomes one dot. CSS pixels are 96 dpi.
pub fn rasterization_scale(layout_mm: u32, width_dots: u32) -> f64 {
    (width_dots as f64) / (layout_mm as f64 * 96.0 / 25.4)
}

pub struct MonoBitmap {
    pub width: u32,
    pub height: u32,
    /// Packed rows, MSB first, one bit per dot. A set bit burns.
    pub bits: Vec<u8>,
}

impl MonoBitmap {
    pub fn stride(&self) -> usize {
        ((self.width + 7) / 8) as usize
    }

    pub fn is_blank(&self) -> bool {
        self.bits.iter().all(|byte| *byte == 0)
    }
}

/// Threshold 8-bit greyscale into 1bpp at `target_width` dots. Source columns past
/// `target_width` are dropped and missing columns stay white.
pub fn pack_luma(
    src_width: u32,
    src_height: u32,
    luma: &[u8],
    target_width: u32,
    threshold: u8,
) -> MonoBitmap {
    let stride = ((target_width + 7) / 8) as usize;
    let mut bits = vec![0u8; stride * src_height as usize];
    let copy_width = src_width.min(target_width);

    for y in 0..src_height {
        let row_start = y as usize * src_width as usize;
        let out_row = y as usize * stride;
        for x in 0..copy_width {
            let Some(value) = luma.get(row_start + x as usize) else {
                continue;
            };
            if *value < threshold {
                bits[out_row + (x / 8) as usize] |= 0x80 >> (x % 8);
            }
        }
    }

    MonoBitmap {
        width: target_width,
        height: src_height,
        bits,
    }
}

/// Drop blank rows at the bottom so a short sale does not eject a viewport of
/// blank paper, and keep a little margin before the cut.
pub fn trim_trailing_blank_rows(bitmap: MonoBitmap, keep_rows: u32) -> MonoBitmap {
    let stride = bitmap.stride();
    if stride == 0 {
        return bitmap;
    }

    let mut last_inked = None;
    for y in (0..bitmap.height).rev() {
        let start = y as usize * stride;
        let row = &bitmap.bits[start..start + stride];
        if row.iter().any(|byte| *byte != 0) {
            last_inked = Some(y);
            break;
        }
    }

    let Some(last_inked) = last_inked else {
        return bitmap;
    };

    let height = (last_inked + 1 + keep_rows).min(bitmap.height);
    let mut bits = bitmap.bits;
    bits.truncate(stride * height as usize);
    MonoBitmap {
        width: bitmap.width,
        height,
        bits,
    }
}

/// Wrap a bitmap in an ESC/POS job: reset, print the bit image in bands, then feed
/// clear of the head and cut once.
pub fn escpos_payload(bitmap: &MonoBitmap) -> Vec<u8> {
    let stride = bitmap.stride();
    let mut out = Vec::with_capacity(bitmap.bits.len() + 256);
    out.extend_from_slice(&[0x1b, 0x40]); // ESC @   — reset

    // `ESC @` restores the printer's saved settings, which on many units includes a
    // non-zero left margin. That offset would push the bitmap right and shove the
    // same number of dots off the right edge, so set the origin and the print area
    // explicitly rather than trusting the reset.
    out.extend_from_slice(&[0x1d, 0x4c, 0x00, 0x00]); // GS L 0 0 — left margin = 0
    let area = bitmap.width.min(0xffff) as u16;
    out.extend_from_slice(&[0x1d, 0x57]); // GS W    — print area width
    out.push((area & 0xff) as u8);
    out.push((area >> 8) as u8);
    out.extend_from_slice(&[0x1b, 0x61, 0x00]); // ESC a 0 — left align
    out.extend_from_slice(&[0x1b, 0x24, 0x00, 0x00]); // ESC $ 0 0 — start at dot 0

    let mut row = 0;
    while row < bitmap.height {
        let rows = BAND_ROWS.min(bitmap.height - row);
        let x_bytes = stride as u32;
        out.extend_from_slice(&[0x1d, 0x76, 0x30, 0x00]); // GS v 0 m=0
        out.push((x_bytes & 0xff) as u8);
        out.push((x_bytes >> 8) as u8);
        out.push((rows & 0xff) as u8);
        out.push((rows >> 8) as u8);
        let start = row as usize * stride;
        out.extend_from_slice(&bitmap.bits[start..start + rows as usize * stride]);
        row += rows;
    }

    out.extend_from_slice(&[0x1b, 0x64, 0x02]); // ESC d 2  — clear the tear bar
    out.extend_from_slice(&[0x1d, 0x56, 0x42, 0x30]); // GS V 66 48 — feed 6mm, cut
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry_uses_printable_width_not_roll_width() {
        assert_eq!(paper_geometry(58), (48, 384));
        assert_eq!(paper_geometry(80), (72, 576));
    }

    /// 8 dots/mm at 203 dpi: the layout width must be exactly the width the head
    /// can cover, or the receipt prints squeezed or clipped.
    #[test]
    fn geometry_layout_width_matches_dot_pitch() {
        for roll in [58, 80] {
            let (layout_mm, dots) = paper_geometry(roll);
            assert_eq!(dots, layout_mm * 8, "{roll}mm roll");
        }
    }

    /// Scale must make the receipt exactly as wide as the head, whatever the roll.
    #[test]
    fn scale_maps_layout_width_onto_head_width() {
        for roll in [58, 80] {
            let (layout_mm, dots) = paper_geometry(roll);
            let scale = rasterization_scale(layout_mm, dots);
            let rendered = (layout_mm as f64 * 96.0 / 25.4) * scale;
            assert!(
                (rendered - dots as f64).abs() < 0.5,
                "{roll}mm rendered {rendered} px for {dots} dots"
            );
            assert!(scale > 1.0 && scale < 3.0, "{roll}mm scale was {scale}");
        }
    }

    /// One CSS mm must print as one physical mm — that is 203/96 device pixels.
    /// Any other value means the receipt is scaled up or down on paper.
    #[test]
    fn scale_is_the_dpi_ratio() {
        let expected = 203.2 / 96.0;
        for roll in [58, 80] {
            let (layout_mm, dots) = paper_geometry(roll);
            let scale = rasterization_scale(layout_mm, dots);
            assert!(
                (scale - expected).abs() < 0.01,
                "{roll}mm scale {scale}, expected about {expected}"
            );
        }
    }

    #[test]
    fn pack_luma_sets_leftmost_dot_in_high_bit() {
        let luma = [0u8, 255, 255, 255, 255, 255, 255, 255];
        let bitmap = pack_luma(8, 1, &luma, 8, BLACK_THRESHOLD);
        assert_eq!(bitmap.bits, vec![0b1000_0000]);
        assert!(!bitmap.is_blank());
    }

    #[test]
    fn pack_luma_pads_narrow_capture_with_white() {
        let luma = [0u8, 0];
        let bitmap = pack_luma(2, 1, &luma, 16, BLACK_THRESHOLD);
        assert_eq!(bitmap.stride(), 2);
        assert_eq!(bitmap.bits, vec![0b1100_0000, 0]);
    }

    #[test]
    fn blank_capture_is_detected() {
        let luma = vec![255u8; 32];
        assert!(pack_luma(8, 4, &luma, 8, BLACK_THRESHOLD).is_blank());
    }

    #[test]
    fn trim_keeps_ink_and_requested_margin() {
        let mut luma = vec![255u8; 8 * 10];
        luma[8 * 2] = 0; // ink on row 2 only
        let bitmap = pack_luma(8, 10, &luma, 8, BLACK_THRESHOLD);
        let trimmed = trim_trailing_blank_rows(bitmap, 3);
        assert_eq!(trimmed.height, 6);
        assert_eq!(trimmed.bits.len(), 6);
    }

    #[test]
    fn trim_leaves_blank_bitmap_alone() {
        let luma = vec![255u8; 8 * 4];
        let trimmed = trim_trailing_blank_rows(pack_luma(8, 4, &luma, 8, BLACK_THRESHOLD), 2);
        assert_eq!(trimmed.height, 4);
    }

    #[test]
    fn payload_bands_long_receipts_and_cuts_once() {
        let height = 300;
        let luma = vec![0u8; 8 * height as usize];
        let bitmap = pack_luma(8, height, &luma, 8, BLACK_THRESHOLD);
        let payload = escpos_payload(&bitmap);

        assert!(payload.starts_with(&[0x1b, 0x40]));
        assert!(payload.ends_with(&[0x1d, 0x56, 0x42, 0x30]));

        let bands = payload
            .windows(4)
            .filter(|w| *w == [0x1d, 0x76, 0x30, 0x00])
            .count();
        assert_eq!(bands, 3, "300 rows should stream as 128 + 128 + 44");

        let cuts = payload
            .windows(4)
            .filter(|w| *w == [0x1d, 0x56, 0x42, 0x30])
            .count();
        assert_eq!(cuts, 1, "exactly one cut per receipt");
    }

    /// A saved left margin is the classic cause of a receipt drifting right with
    /// the right-hand characters missing, so the payload must zero it every time.
    #[test]
    fn payload_zeroes_left_margin_and_claims_full_print_width() {
        let luma = vec![0u8; 576 * 2];
        let bitmap = pack_luma(576, 2, &luma, 576, BLACK_THRESHOLD);
        let payload = escpos_payload(&bitmap);

        let margin_at = payload
            .windows(4)
            .position(|w| w == [0x1d, 0x4c, 0x00, 0x00])
            .expect("GS L 0 0 left margin reset");
        let area_at = payload
            .windows(4)
            .position(|w| w == [0x1d, 0x57, 0x40, 0x02])
            .expect("GS W print area of 576 dots");
        let band_at = payload
            .windows(4)
            .position(|w| w == [0x1d, 0x76, 0x30, 0x00])
            .expect("band header");

        // Origin and width have to be established before any image data.
        assert!(margin_at < band_at, "left margin reset must precede the image");
        assert!(area_at < band_at, "print area must precede the image");
    }

    #[test]
    fn payload_sets_print_area_to_the_narrow_roll_width() {
        let luma = vec![0u8; 384];
        let bitmap = pack_luma(384, 1, &luma, 384, BLACK_THRESHOLD);
        let payload = escpos_payload(&bitmap);
        // 384 = 0x0180 -> nL 0x80, nH 0x01
        assert!(
            payload
                .windows(4)
                .any(|w| w == [0x1d, 0x57, 0x80, 0x01]),
            "58mm print area should be 384 dots"
        );
    }

    #[test]
    fn payload_band_header_carries_width_and_row_count() {
        let luma = vec![0u8; 576 * 2];
        let bitmap = pack_luma(576, 2, &luma, 576, BLACK_THRESHOLD);
        let payload = escpos_payload(&bitmap);
        let at = payload
            .windows(4)
            .position(|w| w == [0x1d, 0x76, 0x30, 0x00])
            .expect("band header");
        // 576 dots = 72 bytes per row, 2 rows.
        assert_eq!(&payload[at + 4..at + 8], &[72, 0, 2, 0]);
    }
}
