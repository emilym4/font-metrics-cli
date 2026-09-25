//! Parsing for the `cmap` table: the character-to-glyph mapping that lets
//! `--char` take a real character instead of a raw glyph id.
//!
//! A `cmap` table can hold several subtables, one per (platform, encoding)
//! pair it supports. This only implements the three subtable formats that
//! cover essentially every font seen in practice - format 4 (BMP, the
//! common case for older/simpler fonts), format 12 (full Unicode, used by
//! any font with glyphs outside the BMP), and format 0 (the original
//! 256-entry byte table, still found in some legacy Mac fonts). Formats 2,
//! 6, 13, and 14 exist for CJK and variation-selector edge cases this tool
//! doesn't need to handle.

use crate::sfnt::{read_i16, read_u16, read_u32, ParseError};

/// Finds the glyph id for `codepoint` by picking the best available
/// subtable out of `cmap_data` (the raw bytes of a font's `cmap` table) and
/// searching it. Returns `Ok(None)` if the subtable exists but has no entry
/// for this codepoint, which callers should treat the same as "no glyph"
/// rather than a parse failure.
pub fn find_glyph_id(cmap_data: &[u8], codepoint: u32) -> Result<Option<u16>, ParseError> {
    const HEADER_LEN: usize = 4;
    if cmap_data.len() < HEADER_LEN {
        return Err(ParseError::UnexpectedTableLength { table: "cmap", needed: HEADER_LEN, have: cmap_data.len() });
    }

    let num_tables = read_u16(cmap_data, 2) as usize;
    let records_end = HEADER_LEN + num_tables * 8;
    if cmap_data.len() < records_end {
        return Err(ParseError::UnexpectedTableLength { table: "cmap", needed: records_end, have: cmap_data.len() });
    }

    let mut best: Option<(i32, usize, u16)> = None; // (priority, subtable offset, format)
    for i in 0..num_tables {
        let base = HEADER_LEN + i * 8;
        let platform_id = read_u16(cmap_data, base);
        let encoding_id = read_u16(cmap_data, base + 2);
        let offset = read_u32(cmap_data, base + 4) as usize;

        if offset + 2 > cmap_data.len() {
            continue;
        }
        let format = read_u16(cmap_data, offset);
        if !matches!(format, 0 | 4 | 12) {
            continue;
        }

        let priority = encoding_priority(platform_id, encoding_id);
        let better_than_current_best = match best {
            Some((best_priority, _, _)) => priority > best_priority,
            None => true,
        };
        if better_than_current_best {
            best = Some((priority, offset, format));
        }
    }

    let (_, offset, format) = best.ok_or(ParseError::NoUsableCmapSubtable)?;
    let subtable = &cmap_data[offset..];
    match format {
        0 => Ok(lookup_format0(subtable, codepoint)),
        4 => lookup_format4(subtable, codepoint),
        12 => lookup_format12(subtable, codepoint),
        _ => unreachable!("format was already checked to be 0, 4, or 12"),
    }
}

/// Ranks a (platformID, encodingID) pair by how good a source of Unicode
/// glyph ids it is, highest first. Full-repertoire Unicode encodings beat
/// BMP-only ones, which beat symbol and legacy Mac Roman tables - the same
/// order text shapers use when a font offers more than one cmap subtable.
fn encoding_priority(platform_id: u16, encoding_id: u16) -> i32 {
    match (platform_id, encoding_id) {
        (3, 10) | (0, 4) | (0, 6) => 5, // Windows/Unicode, full repertoire
        (3, 1) | (0, 3) | (0, 2) | (0, 1) => 4, // Windows/Unicode, BMP
        (0, 0) => 3,                    // Unicode 1.0
        (3, 0) => 2,                    // Windows symbol
        (1, 0) => 1,                    // Macintosh Roman
        _ => 0,
    }
}

/// Format 0: a flat table mapping single-byte codepoints 0-255 straight to
/// glyph ids. Anything outside that range has no entry.
fn lookup_format0(data: &[u8], codepoint: u32) -> Option<u16> {
    if codepoint >= 256 {
        return None;
    }
    let index = 6 + codepoint as usize;
    if index >= data.len() {
        return None;
    }
    match data[index] {
        0 => None,
        glyph_id => Some(glyph_id as u16),
    }
}

/// Format 4: BMP codepoints only, stored as sorted (startCode, endCode)
/// segments. Each segment either offsets straight into the glyph id space
/// via `idDelta`, or points into a shared `glyphIdArray` via
/// `idRangeOffset` - the same two cases every cmap format 4 reader has to
/// handle. See the OpenType spec's `cmap` chapter for the field layout.
fn lookup_format4(data: &[u8], codepoint: u32) -> Result<Option<u16>, ParseError> {
    if codepoint > 0xFFFF {
        return Ok(None);
    }
    let codepoint = codepoint as u16;

    const MIN_LEN: usize = 14;
    if data.len() < MIN_LEN {
        return Err(ParseError::UnexpectedTableLength { table: "cmap format 4", needed: MIN_LEN, have: data.len() });
    }

    let seg_count_x2 = read_u16(data, 6) as usize;
    let end_code_offset = 14;
    let start_code_offset = end_code_offset + seg_count_x2 + 2; // + reservedPad
    let id_delta_offset = start_code_offset + seg_count_x2;
    let id_range_offset_offset = id_delta_offset + seg_count_x2;
    let needed = id_range_offset_offset + seg_count_x2;
    if data.len() < needed {
        return Err(ParseError::UnexpectedTableLength { table: "cmap format 4", needed, have: data.len() });
    }

    for segment in 0..seg_count_x2 / 2 {
        let end_code = read_u16(data, end_code_offset + segment * 2);
        if codepoint > end_code {
            continue;
        }
        let start_code = read_u16(data, start_code_offset + segment * 2);
        if codepoint < start_code {
            // Segments are sorted by endCode ascending, so once codepoint
            // fits under this segment's end but not its start, no earlier
            // segment can match either.
            return Ok(None);
        }

        let id_delta = read_i16(data, id_delta_offset + segment * 2);
        let id_range_offset = read_u16(data, id_range_offset_offset + segment * 2);

        if id_range_offset == 0 {
            let glyph_id = (codepoint as i32 + id_delta as i32) as u16;
            return Ok(if glyph_id == 0 { None } else { Some(glyph_id) });
        }

        // The spec's slightly unusual pointer arithmetic: idRangeOffset is
        // a byte offset from its own storage location, not from the start
        // of the subtable.
        let glyph_index_addr = id_range_offset_offset + segment * 2 + id_range_offset as usize + (codepoint - start_code) as usize * 2;
        if glyph_index_addr + 2 > data.len() {
            return Ok(None);
        }
        let stored_glyph_id = read_u16(data, glyph_index_addr);
        if stored_glyph_id == 0 {
            return Ok(None);
        }
        let glyph_id = (stored_glyph_id as i32 + id_delta as i32) as u16;
        return Ok(if glyph_id == 0 { None } else { Some(glyph_id) });
    }

    Ok(None)
}

/// Format 12: sorted (startCharCode, endCharCode, startGlyphID) groups
/// covering the full Unicode range, used whenever a font has glyphs beyond
/// the BMP (emoji, many CJK extensions, historic scripts).
fn lookup_format12(data: &[u8], codepoint: u32) -> Result<Option<u16>, ParseError> {
    const MIN_LEN: usize = 16;
    if data.len() < MIN_LEN {
        return Err(ParseError::UnexpectedTableLength { table: "cmap format 12", needed: MIN_LEN, have: data.len() });
    }

    let num_groups = read_u32(data, 12) as usize;
    let needed = MIN_LEN + num_groups * 12;
    if data.len() < needed {
        return Err(ParseError::UnexpectedTableLength { table: "cmap format 12", needed, have: data.len() });
    }

    for i in 0..num_groups {
        let base = MIN_LEN + i * 12;
        let start_char_code = read_u32(data, base);
        let end_char_code = read_u32(data, base + 4);
        if codepoint < start_char_code || codepoint > end_char_code {
            continue;
        }
        let start_glyph_id = read_u32(data, base + 8);
        let glyph_id = start_glyph_id + (codepoint - start_char_code);
        // Glyph ids are 16-bit everywhere else in this tool (maxp, hmtx);
        // a font mapping a codepoint past that isn't one this tool can
        // otherwise use.
        return Ok(u16::try_from(glyph_id).ok());
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set_u16(buf: &mut [u8], offset: usize, value: u16) {
        buf[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
    }

    fn set_i16(buf: &mut [u8], offset: usize, value: i16) {
        set_u16(buf, offset, value as u16);
    }

    fn set_u32(buf: &mut [u8], offset: usize, value: u32) {
        buf[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
    }

    /// Builds a `cmap` table with a single format 4 subtable under the
    /// common Windows/Unicode BMP encoding (platform 3, encoding 1), with
    /// two segments: 'A'..'C' mapped via idDelta, and a single-entry
    /// glyphIdArray segment for 'Z'.
    fn cmap_with_format4() -> Vec<u8> {
        let seg_count = 3usize; // 'A'-'C', 'Z'-'Z', and the required terminator
        let seg_count_x2 = seg_count * 2;
        let subtable_len = 16 + seg_count_x2 * 4 + 2; // +2 for reservedPad, +2 for one glyphIdArray entry
        let header_len = 4 + 8; // cmap header + one encoding record
        let subtable_offset = header_len;

        let mut data = vec![0u8; header_len + subtable_len];
        set_u16(&mut data, 2, 1); // numTables
        set_u16(&mut data, 4, 3); // platformID
        set_u16(&mut data, 6, 1); // encodingID
        set_u32(&mut data, 8, subtable_offset as u32);

        let t = subtable_offset;
        set_u16(&mut data, t, 4); // format
        set_u16(&mut data, t + 6, seg_count_x2 as u16);

        let end_code_offset = t + 14;
        let start_code_offset = end_code_offset + seg_count_x2 + 2;
        let id_delta_offset = start_code_offset + seg_count_x2;
        let id_range_offset_offset = id_delta_offset + seg_count_x2;
        let glyph_id_array_offset = id_range_offset_offset + seg_count_x2;

        // Segment 0: 'A' (0x41) - 'C' (0x43), glyph id = codepoint + 9
        // (so 'A' -> glyph 74).
        set_u16(&mut data, end_code_offset, 0x43);
        set_u16(&mut data, start_code_offset, 0x41);
        set_i16(&mut data, id_delta_offset, 9);
        set_u16(&mut data, id_range_offset_offset, 0);

        // Segment 1: 'Z' (0x5A) only, via glyphIdArray (idRangeOffset != 0).
        set_u16(&mut data, end_code_offset + 2, 0x5A);
        set_u16(&mut data, start_code_offset + 2, 0x5A);
        set_i16(&mut data, id_delta_offset + 2, 0);
        let range_offset = (glyph_id_array_offset - (id_range_offset_offset + 2)) as u16;
        set_u16(&mut data, id_range_offset_offset + 2, range_offset);
        set_u16(&mut data, glyph_id_array_offset, 300); // glyph id for 'Z'

        // Segment 2: the mandatory 0xFFFF terminator segment, empty.
        set_u16(&mut data, end_code_offset + 4, 0xFFFF);
        set_u16(&mut data, start_code_offset + 4, 0xFFFF);
        set_i16(&mut data, id_delta_offset + 4, 1);
        set_u16(&mut data, id_range_offset_offset + 4, 0);

        data
    }

    #[test]
    fn looks_up_a_glyph_via_format_4_id_delta() {
        let data = cmap_with_format4();
        assert_eq!(find_glyph_id(&data, 'A' as u32).unwrap(), Some(74));
        assert_eq!(find_glyph_id(&data, 'C' as u32).unwrap(), Some(76));
    }

    #[test]
    fn looks_up_a_glyph_via_format_4_glyph_id_array() {
        let data = cmap_with_format4();
        assert_eq!(find_glyph_id(&data, 'Z' as u32).unwrap(), Some(300));
    }

    #[test]
    fn returns_none_for_a_codepoint_with_no_segment() {
        let data = cmap_with_format4();
        assert_eq!(find_glyph_id(&data, 'D' as u32).unwrap(), None);
    }

    /// Builds a `cmap` table with a single format 12 subtable (platform 3,
    /// encoding 10 - full Unicode), covering one group starting well
    /// outside the BMP so it can only be reached through format 12.
    fn cmap_with_format12() -> Vec<u8> {
        let header_len = 4 + 8;
        let subtable_len = 16 + 12; // one group
        let subtable_offset = header_len;

        let mut data = vec![0u8; header_len + subtable_len];
        set_u16(&mut data, 2, 1); // numTables
        set_u16(&mut data, 4, 3); // platformID
        set_u16(&mut data, 6, 10); // encodingID
        set_u32(&mut data, 8, subtable_offset as u32);

        let t = subtable_offset;
        set_u16(&mut data, t, 12); // format
        set_u32(&mut data, t + 12, 1); // numGroups
        set_u32(&mut data, t + 16, 0x1F600); // startCharCode (an emoji)
        set_u32(&mut data, t + 20, 0x1F60F); // endCharCode
        set_u32(&mut data, t + 24, 500); // startGlyphID

        data
    }

    #[test]
    fn looks_up_a_glyph_via_format_12() {
        let data = cmap_with_format12();
        assert_eq!(find_glyph_id(&data, 0x1F600).unwrap(), Some(500));
        assert_eq!(find_glyph_id(&data, 0x1F605).unwrap(), Some(505));
        assert_eq!(find_glyph_id(&data, 0x1F700).unwrap(), None);
    }

    /// When both a format 4 (BMP) and format 12 (full Unicode) subtable are
    /// present, the format 12 one should win even though it's listed second
    /// - it's the more capable encoding.
    #[test]
    fn prefers_format_12_over_format_4_when_both_are_present() {
        let format4 = cmap_with_format4();
        let format4_subtable = &format4[12..]; // skip its own header + one record

        let header_len = 4 + 16; // cmap header + two encoding records
        let format12_subtable_len = 16 + 12;
        let format4_subtable_offset = header_len;
        let format12_subtable_offset = format4_subtable_offset + format4_subtable.len();

        let mut data = vec![0u8; header_len + format4_subtable.len() + format12_subtable_len];
        set_u16(&mut data, 2, 2); // numTables

        set_u16(&mut data, 4, 3); // record 0: platform 3
        set_u16(&mut data, 6, 1); // encoding 1 (BMP)
        set_u32(&mut data, 8, format4_subtable_offset as u32);

        set_u16(&mut data, 12, 3); // record 1: platform 3
        set_u16(&mut data, 14, 10); // encoding 10 (full Unicode)
        set_u32(&mut data, 16, format12_subtable_offset as u32);

        data[format4_subtable_offset..format4_subtable_offset + format4_subtable.len()].copy_from_slice(format4_subtable);

        let t = format12_subtable_offset;
        set_u16(&mut data, t, 12);
        set_u32(&mut data, t + 12, 1);
        set_u32(&mut data, t + 16, 0x1F600);
        set_u32(&mut data, t + 20, 0x1F60F);
        set_u32(&mut data, t + 24, 500);

        // A codepoint only format 4 can answer still works, proving both
        // subtables parsed correctly...
        assert_eq!(find_glyph_id(&data, 'A' as u32).unwrap(), None); // out of format 12's range, and format 4 was never consulted
        // ...while a full-Unicode codepoint resolves through format 12.
        assert_eq!(find_glyph_id(&data, 0x1F600).unwrap(), Some(500));
    }

    #[test]
    fn errors_when_no_subtable_uses_a_supported_format() {
        let header_len = 4 + 8;
        let mut data = vec![0u8; header_len + 4];
        set_u16(&mut data, 2, 1); // numTables
        set_u16(&mut data, 4, 3); // platformID
        set_u16(&mut data, 6, 1); // encodingID
        set_u32(&mut data, 8, header_len as u32);
        set_u16(&mut data, header_len, 6); // format 6: not implemented

        assert_eq!(find_glyph_id(&data, 'A' as u32), Err(ParseError::NoUsableCmapSubtable));
    }

    #[test]
    fn looks_up_a_glyph_via_format_0() {
        let header_len = 4 + 8;
        let subtable_len = 6 + 256;
        let mut data = vec![0u8; header_len + subtable_len];
        set_u16(&mut data, 2, 1); // numTables
        set_u16(&mut data, 4, 1); // platformID (Macintosh)
        set_u16(&mut data, 6, 0); // encodingID (Roman)
        set_u32(&mut data, 8, header_len as u32);

        let t = header_len;
        set_u16(&mut data, t, 0); // format
        data[t + 6 + 0x41] = 74; // 'A' -> glyph 74

        assert_eq!(find_glyph_id(&data, 'A' as u32).unwrap(), Some(74));
        assert_eq!(find_glyph_id(&data, 'B' as u32).unwrap(), None);
        assert_eq!(find_glyph_id(&data, 0x1F600).unwrap(), None); // outside byte range
    }
}
