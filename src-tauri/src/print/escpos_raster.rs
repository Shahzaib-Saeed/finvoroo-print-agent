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

/// Luma below this burns a dot. A higher cutoff turns anti-aliased logo edges
/// and small glyphs solid black, which weaker heads (Black Copper) can actually
/// mark. Dithering is still avoided — grey mush on thermal paper looks faded.
pub const BLACK_THRESHOLD: u8 = 208;

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
        // Classic 72mm / 576-dot printable band. Wide 640-dot heads (Black Copper,
        // Xprinter, generic POS-80) then centre this band in a 640-dot payload so
        // gutters stay equal and the ticket fills the roll.
        (72, 576)
    }
}

/// Premium heads (Bixolon / Epson TM / Star) fill a 72mm (576-dot) band correctly.
/// Most other 80mm POS printers (Black Copper, Xprinter, Rongta, generic POS-80, …)
/// left-align a 576-dot job and leave empty paper on the right — they need a
/// full 80mm / 640-dot raster. Universal default is full-width; only known
/// narrow-band brands keep 576.
pub fn is_narrow_band_80mm_head(printer: &str) -> bool {
    let hay = printer.to_ascii_lowercase().replace('_', " ");
    // Bixolon SRP / BC-95 (Bixolon OEM). Do not match Black Copper BC-96.
    if hay.contains("bixolon")
        || hay.contains("srp-")
        || hay.contains("srp ")
        || hay.contains("srp330")
        || hay.contains("srp350")
        || hay.contains("srp382")
        || hay.contains("bc-95")
        || hay.contains("bc95")
    {
        return true;
    }
    // Epson TM thermal, Star Micronics, Citizen CT — classic 576-dot ESC/POS.
    if hay.contains("epson")
        || hay.contains("tm-t")
        || hay.contains("tm-m")
        || hay.contains("tm-u")
        || hay.contains("tm-p")
        || hay.contains("star micronics")
        || hay.contains("star tsp")
        || hay.contains("tsp100")
        || hay.contains("tsp143")
        || hay.contains("citizen ct")
        || hay.contains("citizen cbm")
    {
        return true;
    }
    false
}

/// @deprecated Prefer [`is_narrow_band_80mm_head`] — wide-head is now the default.
pub fn is_wide_80mm_left_align_head(printer: &str) -> bool {
    !is_narrow_band_80mm_head(printer)
}

/// Geometry for the named printer.
///
/// Every 80mm head burns a 72mm (576-dot) printable band. Layout wider than
/// that (80mm / 640 dots) clips the right column on Black Copper, Xprinter, and
/// most generic POS-80 units — the band is then centred in a 640-dot payload.
pub fn paper_geometry_for_printer(paper_mm: u32, _printer: &str) -> (u32, u32) {
    paper_geometry(paper_mm)
}

/// ESC/POS payload width — must match the layout band (576 dots on 80mm).
/// Padding a 576-dot capture into a 640-dot canvas clips the right column on
/// Black Copper / generic POS-80 heads that only burn the first 576 dots.
pub fn payload_width_dots(paper_mm: u32, _printer: &str) -> u32 {
    paper_geometry(paper_mm).1
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

/// Drop blank columns on the left so a mis-sized capture does not drift right on paper.
pub fn trim_leading_blank_columns(bitmap: MonoBitmap) -> MonoBitmap {
    let stride = bitmap.stride();
    if stride == 0 || bitmap.width == 0 {
        return bitmap;
    }

    let mut first_col: Option<u32> = None;
    'search: for x in 0..bitmap.width {
        for y in 0..bitmap.height {
            let byte_idx = y as usize * stride + (x / 8) as usize;
            let mask = 0x80u8 >> (x % 8);
            if bitmap.bits.get(byte_idx).map(|b| b & mask != 0).unwrap_or(false) {
                first_col = Some(x);
                break 'search;
            }
        }
    }

    let Some(col) = first_col else {
        return bitmap;
    };
    if col == 0 {
        return bitmap;
    }

    let new_width = bitmap.width - col;
    let new_stride = ((new_width + 7) / 8) as usize;
    let mut new_bits = vec![0u8; new_stride * bitmap.height as usize];

    for y in 0..bitmap.height {
        for x in col..bitmap.width {
            let src_byte = y as usize * stride + (x / 8) as usize;
            let src_mask = 0x80u8 >> (x % 8);
            if bitmap.bits[src_byte] & src_mask == 0 {
                continue;
            }
            let dx = x - col;
            let dst_byte = y as usize * new_stride + (dx / 8) as usize;
            let dst_mask = 0x80u8 >> (dx % 8);
            new_bits[dst_byte] |= dst_mask;
        }
    }

    MonoBitmap {
        width: new_width,
        height: bitmap.height,
        bits: new_bits,
    }
}

/// Per-printer left nudge — disabled. Universal layout centres the band in a
/// 640-dot payload instead of shifting per Windows driver name.
pub fn left_margin_nudge_dots(_printer: &str, _paper_mm: u32) -> u32 {
    0
}

/// Expand (or crop) a bitmap to exactly the head width, left-aligned. Many thermal
/// heads centre images that are narrower than the print area, which looks like a
/// right shift on 80 mm paper.
pub fn pad_bitmap_to_head(bitmap: MonoBitmap, head_width_dots: u32) -> MonoBitmap {
    if bitmap.width == 0 || bitmap.height == 0 || head_width_dots == 0 {
        return bitmap;
    }

    if bitmap.width > head_width_dots {
        let stride = bitmap.stride();
        let new_stride = ((head_width_dots + 7) / 8) as usize;
        let mut new_bits = vec![0u8; new_stride * bitmap.height as usize];
        for y in 0..bitmap.height {
            let old_row = y as usize * stride;
            let new_row = y as usize * new_stride;
            new_bits[new_row..new_row + new_stride]
                .copy_from_slice(&bitmap.bits[old_row..old_row + new_stride]);
        }
        return MonoBitmap {
            width: head_width_dots,
            height: bitmap.height,
            bits: new_bits,
        };
    }

    if bitmap.width == head_width_dots {
        return bitmap;
    }

    let old_stride = bitmap.stride();
    let new_stride = ((head_width_dots + 7) / 8) as usize;
    let mut new_bits = vec![0u8; new_stride * bitmap.height as usize];
    for y in 0..bitmap.height {
        let old_row = y as usize * old_stride;
        let new_row = y as usize * new_stride;
        let copy_bytes = old_stride.min(new_stride);
        new_bits[new_row..new_row + copy_bytes]
            .copy_from_slice(&bitmap.bits[old_row..old_row + copy_bytes]);
    }

    MonoBitmap {
        width: head_width_dots,
        height: bitmap.height,
        bits: new_bits,
    }
}

/// Drop blank columns on the right (mirror of [`trim_leading_blank_columns`]).
pub fn trim_trailing_blank_columns(bitmap: MonoBitmap) -> MonoBitmap {
    let stride = bitmap.stride();
    if stride == 0 || bitmap.width == 0 {
        return bitmap;
    }

    let mut last_col: Option<u32> = None;
    'search: for x in (0..bitmap.width).rev() {
        for y in 0..bitmap.height {
            let byte_idx = y as usize * stride + (x / 8) as usize;
            let mask = 0x80u8 >> (x % 8);
            if bitmap.bits.get(byte_idx).map(|b| b & mask != 0).unwrap_or(false) {
                last_col = Some(x);
                break 'search;
            }
        }
    }

    let Some(last) = last_col else {
        return bitmap;
    };
    if last + 1 >= bitmap.width {
        return bitmap;
    }

    let new_width = last + 1;
    let new_stride = ((new_width + 7) / 8) as usize;
    let mut new_bits = vec![0u8; new_stride * bitmap.height as usize];

    for y in 0..bitmap.height {
        for x in 0..new_width {
            let src_byte = y as usize * stride + (x / 8) as usize;
            let src_mask = 0x80u8 >> (x % 8);
            if bitmap.bits[src_byte] & src_mask == 0 {
                continue;
            }
            let dst_byte = y as usize * new_stride + (x / 8) as usize;
            let dst_mask = 0x80u8 >> (x % 8);
            new_bits[dst_byte] |= dst_mask;
        }
    }

    MonoBitmap {
        width: new_width,
        height: bitmap.height,
        bits: new_bits,
    }
}

/// Rows that are mostly ink across the width (table rules, dividers) must not
/// define the horizontal bounds — they span the full capture and block centering.
fn is_full_width_rule_row(row: &[u8], width: u32) -> bool {
    if width == 0 {
        return false;
    }
    row_dot_count(row) * 100 >= width * 55
}

/// Left/right ink bounds using text rows only, ignoring full-width horizontal rules.
fn content_ink_column_bounds(bitmap: &MonoBitmap) -> Option<(u32, u32)> {
    let stride = bitmap.stride();
    if stride == 0 || bitmap.width == 0 || bitmap.height == 0 {
        return None;
    }

    let mut min_x: Option<u32> = None;
    let mut max_x: Option<u32> = None;

    for y in 0..bitmap.height {
        let start = y as usize * stride;
        let row = &bitmap.bits[start..start + stride];
        if is_full_width_rule_row(row, bitmap.width) {
            continue;
        }

        for x in 0..bitmap.width {
            let byte_idx = start + (x / 8) as usize;
            let mask = 0x80u8 >> (x % 8);
            if bitmap.bits.get(byte_idx).map(|b| b & mask != 0).unwrap_or(false) {
                min_x = Some(min_x.map_or(x, |m| m.min(x)));
                max_x = Some(max_x.map_or(x, |m| m.max(x)));
            }
        }
    }

    match (min_x, max_x) {
        (Some(lo), Some(hi)) if hi >= lo => Some((lo, hi + 1)),
        _ => None,
    }
}

/// Trim capture gutters; centre only when the ink band is narrower than the head.
pub fn center_content_on_head(bitmap: MonoBitmap, head_width_dots: u32) -> MonoBitmap {
    let trimmed = trim_trailing_blank_columns(trim_leading_blank_columns(bitmap));
    if trimmed.width >= head_width_dots {
        return pad_bitmap_to_head(trimmed, head_width_dots);
    }
    pad_bitmap_to_head_centered(trimmed, head_width_dots)
}

fn crop_columns(bitmap: MonoBitmap, left: u32, right: u32) -> MonoBitmap {
    if left >= right || left >= bitmap.width {
        return bitmap;
    }
    let right = right.min(bitmap.width);
    let new_width = right - left;
    if new_width == 0 {
        return bitmap;
    }
    if left == 0 && new_width == bitmap.width {
        return bitmap;
    }

    let stride = bitmap.stride();
    let new_stride = ((new_width + 7) / 8) as usize;
    let mut new_bits = vec![0u8; new_stride * bitmap.height as usize];

    for y in 0..bitmap.height {
        for x in left..right {
            let src_byte = y as usize * stride + (x / 8) as usize;
            let src_mask = 0x80u8 >> (x % 8);
            if bitmap.bits[src_byte] & src_mask == 0 {
                continue;
            }
            let dx = x - left;
            let dst_byte = y as usize * new_stride + (dx / 8) as usize;
            let dst_mask = 0x80u8 >> (dx % 8);
            new_bits[dst_byte] |= dst_mask;
        }
    }

    MonoBitmap {
        width: new_width,
        height: bitmap.height,
        bits: new_bits,
    }
}

/// Like [`pad_bitmap_to_head`], but centres a narrower capture in the head width
/// so leftover paper is split left/right (Black Copper left-aligns otherwise).
pub fn pad_bitmap_to_head_centered(bitmap: MonoBitmap, head_width_dots: u32) -> MonoBitmap {
    if bitmap.width == 0 || bitmap.height == 0 || head_width_dots == 0 {
        return bitmap;
    }
    if bitmap.width >= head_width_dots {
        return pad_bitmap_to_head(bitmap, head_width_dots);
    }

    let inset = (head_width_dots - bitmap.width) / 2;
    let old_stride = bitmap.stride();
    let new_stride = ((head_width_dots + 7) / 8) as usize;
    let mut new_bits = vec![0u8; new_stride * bitmap.height as usize];

    for y in 0..bitmap.height {
        for x in 0..bitmap.width {
            let src_byte = y as usize * old_stride + (x / 8) as usize;
            let src_mask = 0x80u8 >> (x % 8);
            if bitmap.bits.get(src_byte).map(|b| b & src_mask != 0).unwrap_or(false) {
                let dx = x + inset;
                let dst_byte = y as usize * new_stride + (dx / 8) as usize;
                let dst_mask = 0x80u8 >> (dx % 8);
                new_bits[dst_byte] |= dst_mask;
            }
        }
    }

    MonoBitmap {
        width: head_width_dots,
        height: bitmap.height,
        bits: new_bits,
    }
}

/// Move content left by `dots`, filling the right with white. Used to cancel a
/// printer leftover left margin. Must not exceed the CSS side gutter.
pub fn shift_content_left(bitmap: MonoBitmap, dots: u32) -> MonoBitmap {
    if dots == 0 || bitmap.width <= dots {
        return bitmap;
    }

    let orig_width = bitmap.width;
    let stride = bitmap.stride();
    let new_width = bitmap.width - dots;
    let new_stride = ((new_width + 7) / 8) as usize;
    let mut new_bits = vec![0u8; new_stride * bitmap.height as usize];

    for y in 0..bitmap.height {
        for x in dots..bitmap.width {
            let src_byte = y as usize * stride + (x / 8) as usize;
            let src_mask = 0x80u8 >> (x % 8);
            if bitmap.bits.get(src_byte).map(|b| b & src_mask != 0).unwrap_or(false) {
                let dx = x - dots;
                let dst_byte = y as usize * new_stride + (dx / 8) as usize;
                let dst_mask = 0x80u8 >> (dx % 8);
                new_bits[dst_byte] |= dst_mask;
            }
        }
    }

    pad_bitmap_to_head(
        MonoBitmap {
            width: new_width,
            height: bitmap.height,
            bits: new_bits,
        },
        orig_width,
    )
}

/// Drop blank rows at the top so captured HTML whitespace does not feed out as paper.
pub fn trim_leading_blank_rows(bitmap: MonoBitmap) -> MonoBitmap {
    let stride = bitmap.stride();
    if stride == 0 {
        return bitmap;
    }

    let mut first_inked = None;
    'rows: for y in 0..bitmap.height {
        let start = y as usize * stride;
        let row = &bitmap.bits[start..start + stride];
        if row_dot_count(row) >= TRIM_TRAILING_MIN_DOTS {
            first_inked = Some(y);
            break 'rows;
        }
    }

    let Some(first_inked) = first_inked else {
        return bitmap;
    };

    if first_inked == 0 {
        return bitmap;
    }

    let new_height = bitmap.height - first_inked;
    let start = first_inked as usize * stride;
    MonoBitmap {
        width: bitmap.width,
        height: new_height,
        bits: bitmap.bits[start..].to_vec(),
    }
}

/// Ignore JPEG/capture speckle — real receipt rules and text burn many more dots.
const TRIM_TRAILING_MIN_DOTS: u32 = 4;

fn row_dot_count(row: &[u8]) -> u32 {
    row.iter().map(|b| b.count_ones()).sum()
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
        if row_dot_count(row) >= TRIM_TRAILING_MIN_DOTS {
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

/// Append pure-white rows after the last ink so the cut sits clear of branding.
/// [`trim_trailing_blank_rows`] alone cannot invent space when the capture ends
/// on the last inked row (common with `padding-bottom: 0`).
pub fn append_blank_rows(bitmap: MonoBitmap, rows: u32) -> MonoBitmap {
    if rows == 0 || bitmap.width == 0 {
        return bitmap;
    }
    let stride = bitmap.stride();
    let mut bits = bitmap.bits;
    bits.resize(bits.len() + stride * rows as usize, 0);
    MonoBitmap {
        width: bitmap.width,
        height: bitmap.height + rows,
        bits,
    }
}

/// Default feed units before partial cut (`GS V 66 n`). ~1 unit ≈ 0.125mm at 203dpi.
pub const CUT_FEED_UNITS_DEFAULT: u8 = 48;
/// Extra feed for heads that cut close to the print line (~10mm).
pub const CUT_FEED_UNITS_WIDE_HEAD: u8 = 80;
/// White rows after branding for every printer (~6mm @ 8 dots/mm).
pub const TRAILING_BLANK_ROWS_DEFAULT: u32 = 48;
/// Extra white rows for left-align / short head-to-cutter printers (~8mm).
pub const TRAILING_BLANK_ROWS_WIDE_HEAD: u32 = 64;

/// Zero left margin and claim the full head width before raster data. Bixolon units
/// often restore a saved NV margin on `ESC @`, so margin and width are asserted twice.
fn write_escpos_init(out: &mut Vec<u8>, head_width_dots: u32) {
    out.extend_from_slice(&[0x1b, 0x40]); // ESC @   — reset
    out.extend_from_slice(&[0x1b, 0x4d, 0x00]); // ESC M 0 — standard mode
    out.extend_from_slice(&[0x1b, 0x4a, 0x00]); // ESC J 0 — no feed after reset
    out.extend_from_slice(&[0x1d, 0x4c, 0x00, 0x00]); // GS L 0 0 — left margin = 0
    out.extend_from_slice(&[0x1d, 0x4c, 0x00, 0x00]); // GS L 0 0 — re-assert (Bixolon NV)
    let area = head_width_dots.min(0xffff) as u16;
    out.extend_from_slice(&[0x1d, 0x57]); // GS W    — print area = full head width
    out.push((area & 0xff) as u8);
    out.push((area >> 8) as u8);
    out.extend_from_slice(&[0x1b, 0x61, 0x00]); // ESC a 0 — left align
    out.extend_from_slice(&[0x1b, 0x24, 0x00, 0x00]); // ESC $ 0 0 — start at dot 0
}

/// Wrap a bitmap in an ESC/POS job: reset, print the bit image in bands, then feed
/// clear of the head and cut once.
pub fn escpos_payload(bitmap: &MonoBitmap, head_width_dots: u32) -> Vec<u8> {
    escpos_payload_with_cut_feed(bitmap, head_width_dots, CUT_FEED_UNITS_DEFAULT)
}

/// Like [`escpos_payload`], with an explicit feed before the partial cut.
pub fn escpos_payload_with_cut_feed(
    bitmap: &MonoBitmap,
    head_width_dots: u32,
    cut_feed_units: u8,
) -> Vec<u8> {
    let stride = bitmap.stride();
    let mut out = Vec::with_capacity(bitmap.bits.len() + 256);
    write_escpos_init(&mut out, head_width_dots);
    // Bixolon restores NV left margin between init and the first raster band.
    out.extend_from_slice(&[0x1d, 0x4c, 0x00, 0x00]); // GS L 0 0
    out.extend_from_slice(&[0x1b, 0x24, 0x00, 0x00]); // ESC $ 0 0

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

    // GS V 66 n — feed n units past the last dot row, then partial cut.
    out.extend_from_slice(&[0x1d, 0x56, 0x42, cut_feed_units]);
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

    #[test]
    fn layout_and_payload_both_use_576_dot_band_on_80mm() {
        for printer in [
            "Black Copper POS-80",
            "BlackCopper BC-96AC",
            "Xprinter XP-N160II",
            "Bixolon SRP-350plusIII",
            "Epson TM-T88VI",
        ] {
            assert_eq!(
                paper_geometry_for_printer(80, printer),
                (72, 576),
                "{printer}"
            );
            assert_eq!(payload_width_dots(80, printer), 576, "{printer}");
        }
        assert_eq!(
            paper_geometry_for_printer(58, "Black Copper POS-80"),
            (48, 384)
        );
        assert_eq!(payload_width_dots(58, "Black Copper POS-80"), 384);
    }

    #[test]
    fn center_content_does_not_pad_when_capture_matches_head_width() {
        // Full-width 576-dot capture must stay left-aligned — no 640-dot inset.
        let luma = vec![0u8; 576];
        let bitmap = pack_luma(576, 1, &luma, 576, BLACK_THRESHOLD);
        let out = center_content_on_head(bitmap, 576);
        assert_eq!(out.width, 576);
        assert_eq!(out.bits, vec![0xff; 72]); // one solid row, 576 dots
    }

    #[test]
    fn center_content_on_head_balances_gutters_on_640() {
        // 8 black dots in a 16-wide canvas with 4 white on each side → crop to
        // ink then centre in 32 → equal 12-dot gutters.
        let mut luma = vec![255u8; 16];
        for i in 4..12 {
            luma[i] = 0;
        }
        let bitmap = pack_luma(16, 1, &luma, 16, BLACK_THRESHOLD);
        let centered = center_content_on_head(bitmap, 32);
        assert_eq!(centered.width, 32);
        // Columns 12..20 should be black (8 dots centred in 32).
        for x in 0..32u32 {
            let byte = (x / 8) as usize;
            let mask = 0x80u8 >> (x % 8);
            let ink = centered.bits[byte] & mask != 0;
            assert_eq!(ink, (12..20).contains(&x), "col {x}");
        }
    }

    #[test]
    fn narrow_band_detector_only_matches_known_premium_heads() {
        assert!(is_narrow_band_80mm_head("Bixolon SRP-350plusIII"));
        assert!(is_narrow_band_80mm_head("Bixolon BC-95AC"));
        assert!(is_narrow_band_80mm_head("Epson TM-T20III"));
        assert!(!is_narrow_band_80mm_head("Black Copper POS-80"));
        assert!(!is_narrow_band_80mm_head("BC-96AC"));
        assert!(!is_narrow_band_80mm_head("BC86"));
        assert!(!is_narrow_band_80mm_head("Xprinter XP-N160II"));
        assert!(!is_narrow_band_80mm_head("POS-80 USB"));
        // Wide-head helper is the inverse.
        assert!(is_wide_80mm_left_align_head("BC-96AC"));
        assert!(!is_wide_80mm_left_align_head("Bixolon SRP-350plusIII"));
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
    fn grey_logo_edges_burn_instead_of_fading() {
        let luma = [200u8, 207, 220, 255];
        let bitmap = pack_luma(4, 1, &luma, 8, BLACK_THRESHOLD);
        // 200 and 207 are below 208 — typical anti-aliased logo ink.
        assert_eq!(bitmap.bits[0] & 0b1100_0000, 0b1100_0000);
        assert_eq!(bitmap.bits[0] & 0b0011_0000, 0);
    }

    #[test]
    fn left_margin_nudge_is_disabled_for_universal_layout() {
        assert_eq!(left_margin_nudge_dots("Bixolon SRP-350plusIII", 80), 0);
        assert_eq!(left_margin_nudge_dots("Black Copper POS-80", 80), 0);
        assert_eq!(left_margin_nudge_dots("Xprinter XP-N160II", 58), 0);
    }

    #[test]
    fn shift_content_left_moves_ink_without_changing_width() {
        // 16 dots wide, ink in the rightmost 8 columns.
        let mut luma = vec![255u8; 16];
        for i in 8..16 {
            luma[i] = 0;
        }
        let bitmap = pack_luma(16, 1, &luma, 16, BLACK_THRESHOLD);
        let shifted = shift_content_left(bitmap, 8);
        assert_eq!(shifted.width, 16);
        assert_eq!(shifted.bits[0], 0b1111_1111);
        assert_eq!(shifted.bits[1], 0);
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
        for i in 0..8 {
            luma[8 * 2 + i] = 0; // solid rule on row 2
        }
        let bitmap = pack_luma(8, 10, &luma, 8, BLACK_THRESHOLD);
        let trimmed = trim_trailing_blank_rows(bitmap, 3);
        assert_eq!(trimmed.height, 6);
        assert_eq!(trimmed.bits.len(), 6);
    }

    #[test]
    fn trim_leading_drops_top_whitespace() {
        let mut luma = vec![255u8; 8 * 10];
        for i in 0..8 {
            luma[8 * 7 + i] = 0; // solid rule on row 7
        }
        let bitmap = pack_luma(8, 10, &luma, 8, BLACK_THRESHOLD);
        let trimmed = trim_leading_blank_rows(bitmap);
        assert_eq!(trimmed.height, 3);
        assert_eq!(trimmed.bits.len(), 3);
    }

    #[test]
    fn pad_bitmap_to_head_left_aligns_narrow_capture() {
        let luma = vec![0u8; 8 * 2];
        let bitmap = pack_luma(8, 2, &luma, 8, BLACK_THRESHOLD);
        let padded = pad_bitmap_to_head(bitmap, 16);
        assert_eq!(padded.width, 16);
        assert_eq!(padded.stride(), 2);
        assert_eq!(padded.bits.len(), 4);
    }

    #[test]
    fn pad_bitmap_to_head_centered_splits_leftover() {
        let luma = vec![0u8; 8];
        let bitmap = pack_luma(8, 1, &luma, 8, BLACK_THRESHOLD);
        let padded = pad_bitmap_to_head_centered(bitmap, 16);
        assert_eq!(padded.width, 16);
        // 4-dot inset → first 4 dots white, then 8 black, then 4 white.
        assert_eq!(padded.bits[0] & 0b1111_0000, 0);
        assert_eq!(padded.bits[0] & 0b0000_1111, 0b0000_1111);
        assert_eq!(padded.bits[1] & 0b1111_0000, 0b1111_0000);
        assert_eq!(padded.bits[1] & 0b0000_1111, 0);
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
        let payload = escpos_payload(&bitmap, 8);

        assert!(payload.starts_with(&[0x1b, 0x40]));
        assert!(payload.ends_with(&[0x1d, 0x56, 0x42, CUT_FEED_UNITS_DEFAULT]));

        let bands = payload
            .windows(4)
            .filter(|w| *w == [0x1d, 0x76, 0x30, 0x00])
            .count();
        assert_eq!(bands, 3, "300 rows should stream as 128 + 128 + 44");

        let cuts = payload
            .windows(4)
            .filter(|w| *w == [0x1d, 0x56, 0x42, CUT_FEED_UNITS_DEFAULT])
            .count();
        assert_eq!(cuts, 1, "exactly one cut per receipt");
    }

    /// A saved left margin is the classic cause of a receipt drifting right with
    /// the right-hand characters missing, so the payload must zero it every time.
    #[test]
    fn payload_zeroes_left_margin_and_claims_full_print_width() {
        let luma = vec![0u8; 576 * 2];
        let bitmap = pack_luma(576, 2, &luma, 576, BLACK_THRESHOLD);
        let payload = escpos_payload(&bitmap, 576);

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
        let payload = escpos_payload(&bitmap, 384);
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
        let payload = escpos_payload(&bitmap, 576);
        let at = payload
            .windows(4)
            .position(|w| w == [0x1d, 0x76, 0x30, 0x00])
            .expect("band header");
        // 576 dots = 72 bytes per row, 2 rows.
        assert_eq!(&payload[at + 4..at + 8], &[72, 0, 2, 0]);
    }
}
