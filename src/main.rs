use std::env;
use std::fs;
use std::process::ExitCode;

mod sfnt;
mod tables;

use sfnt::ParseError;
use tables::{HeadTable, HheaTable, Os2Table};

fn main() -> ExitCode {
    let mut path = None;
    let mut font_index = 0usize;

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--index" | "-i" => {
                let value = match args.next() {
                    Some(v) => v,
                    None => {
                        eprintln!("fontmetrics: --index needs a number");
                        return ExitCode::FAILURE;
                    }
                };
                font_index = match value.parse() {
                    Ok(n) => n,
                    Err(_) => {
                        eprintln!("fontmetrics: --index must be a non-negative integer, got '{value}'");
                        return ExitCode::FAILURE;
                    }
                };
            }
            _ if path.is_none() => path = Some(arg),
            _ => {
                eprintln!("usage: fontmetrics [--index N] <font-file>");
                return ExitCode::FAILURE;
            }
        }
    }

    let path = match path {
        Some(p) => p,
        None => {
            eprintln!("usage: fontmetrics [--index N] <font-file>");
            return ExitCode::FAILURE;
        }
    };

    let data = match fs::read(&path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("fontmetrics: cannot read {path}: {e}");
            return ExitCode::FAILURE;
        }
    };

    match build_report(&data, font_index) {
        Ok(report) => {
            print!("{report}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("fontmetrics: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Parses the font bytes and renders a human-readable metrics report. Kept
/// separate from `main` so it can be exercised directly with in-memory
/// bytes instead of files on disk. `font_index` selects which font to read
/// out of a `.ttc` collection; plain single-font files only accept 0.
fn build_report(data: &[u8], font_index: usize) -> Result<String, ParseError> {
    let directory = sfnt::parse_font(data, font_index)?;

    let head_record = sfnt::find_table(&directory, b"head").ok_or(ParseError::MissingTable("head"))?;
    let head = tables::parse_head(sfnt::table_bytes(data, &head_record)?)?;

    let hhea_record = sfnt::find_table(&directory, b"hhea").ok_or(ParseError::MissingTable("hhea"))?;
    let hhea = tables::parse_hhea(sfnt::table_bytes(data, &hhea_record)?)?;

    let os2 = match sfnt::find_table(&directory, b"OS/2") {
        Some(record) => Some(tables::parse_os2(sfnt::table_bytes(data, &record)?)?),
        None => None,
    };

    Ok(format_report(&head, &hhea, os2.as_ref()))
}

fn format_report(head: &HeadTable, hhea: &HheaTable, os2: Option<&Os2Table>) -> String {
    let mut out = String::new();

    out.push_str(&format!("units per em:     {}\n", head.units_per_em));
    out.push_str(&format!("hhea ascender:    {}\n", hhea.ascender));
    out.push_str(&format!("hhea descender:   {}\n", hhea.descender));
    out.push_str(&format!("hhea line gap:    {}\n", hhea.line_gap));

    match os2 {
        Some(os2) => {
            // Very old OS/2 version 0 tables (68 bytes, predating
            // usWinAscent/usWinDescent) leave these fields unset. Fall back
            // to the hhea numbers, which cover the same ground.
            let typo_ascender = os2.typo_ascender.unwrap_or(hhea.ascender);
            let typo_descender = os2.typo_descender.unwrap_or(hhea.descender);
            let typo_line_gap = os2.typo_line_gap.unwrap_or(hhea.line_gap);
            let win_ascent = os2.win_ascent.unwrap_or_else(|| hhea.ascender.max(0) as u16);
            let win_descent = os2.win_descent.unwrap_or_else(|| hhea.descender.unsigned_abs());

            out.push_str(&format!("typo ascender:    {typo_ascender}\n"));
            out.push_str(&format!("typo descender:   {typo_descender}\n"));
            out.push_str(&format!("typo line gap:    {typo_line_gap}\n"));
            out.push_str(&format!("win ascent:       {win_ascent}\n"));
            out.push_str(&format!("win descent:      {win_descent}\n"));
            if os2.typo_ascender.is_none() {
                out.push_str("  (typo/win fields above are from hhea: this font's OS/2 table is the old version 0 layout without them)\n");
            }
            if let (Some(cap), Some(x)) = (os2.cap_height, os2.x_height) {
                out.push_str(&format!("cap height:       {cap}\n"));
                out.push_str(&format!("x-height:         {x}\n"));
            }
        }
        None => out.push_str("OS/2 table:       not present\n"),
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set_u32(buf: &mut [u8], offset: usize, value: u32) {
        buf[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
    }

    fn set_u16(buf: &mut [u8], offset: usize, value: u16) {
        buf[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
    }

    fn set_i16(buf: &mut [u8], offset: usize, value: i16) {
        set_u16(buf, offset, value as u16);
    }

    fn write_table_record(dir: &mut [u8], index: usize, tag: &[u8; 4], offset: u32, length: u32) {
        let base = 12 + index * 16;
        dir[base..base + 4].copy_from_slice(tag);
        set_u32(dir, base + 8, offset);
        set_u32(dir, base + 12, length);
    }

    /// Builds a minimal but valid sfnt file with just `head` and `hhea`,
    /// enough to exercise the report end to end without touching disk.
    fn minimal_font() -> Vec<u8> {
        let head_len = 46;
        let hhea_len = 36;
        let head_offset = 12 + 2 * 16;
        let hhea_offset = head_offset + head_len;

        let mut data = vec![0u8; hhea_offset + hhea_len];
        set_u32(&mut data, 0, 0x0001_0000);
        set_u16(&mut data, 4, 2);

        write_table_record(&mut data, 0, b"head", head_offset as u32, head_len as u32);
        write_table_record(&mut data, 1, b"hhea", hhea_offset as u32, hhea_len as u32);

        set_u16(&mut data, head_offset + 18, 1000); // unitsPerEm
        set_i16(&mut data, hhea_offset + 4, 800); // ascender
        set_i16(&mut data, hhea_offset + 6, -200); // descender
        set_i16(&mut data, hhea_offset + 8, 0); // lineGap

        data
    }

    #[test]
    fn reports_metrics_from_a_minimal_font_without_os2() {
        let data = minimal_font();
        let report = build_report(&data, 0).unwrap();

        assert!(report.contains("units per em:     1000"));
        assert!(report.contains("hhea ascender:    800"));
        assert!(report.contains("hhea descender:   -200"));
        assert!(report.contains("OS/2 table:       not present"));
    }

    #[test]
    fn fails_with_a_clear_error_when_head_is_missing() {
        let mut data = vec![0u8; 12];
        set_u32(&mut data, 0, 0x0001_0000);
        set_u16(&mut data, 4, 0);

        assert_eq!(build_report(&data, 0), Err(ParseError::MissingTable("head")));
    }

    /// Builds a font like `minimal_font`, but with an extra, legacy-shaped
    /// (68-byte) OS/2 version 0 table that has no typo or win metrics.
    fn font_with_legacy_os2() -> Vec<u8> {
        let head_len = 46;
        let hhea_len = 36;
        let os2_len = 68;
        let head_offset = 12 + 3 * 16;
        let hhea_offset = head_offset + head_len;
        let os2_offset = hhea_offset + hhea_len;

        let mut data = vec![0u8; os2_offset + os2_len];
        set_u32(&mut data, 0, 0x0001_0000);
        set_u16(&mut data, 4, 3);

        write_table_record(&mut data, 0, b"head", head_offset as u32, head_len as u32);
        write_table_record(&mut data, 1, b"hhea", hhea_offset as u32, hhea_len as u32);
        write_table_record(&mut data, 2, b"OS/2", os2_offset as u32, os2_len as u32);

        set_u16(&mut data, head_offset + 18, 1000); // unitsPerEm
        set_i16(&mut data, hhea_offset + 4, 800); // ascender
        set_i16(&mut data, hhea_offset + 6, -200); // descender
        set_i16(&mut data, hhea_offset + 8, 0); // lineGap
        set_u16(&mut data, os2_offset, 0); // OS/2 version

        data
    }

    #[test]
    fn falls_back_to_hhea_for_a_legacy_os2_version_0_table() {
        let data = font_with_legacy_os2();
        let report = build_report(&data, 0).unwrap();

        assert!(report.contains("typo ascender:    800"));
        assert!(report.contains("typo descender:   -200"));
        assert!(report.contains("win ascent:       800"));
        assert!(report.contains("win descent:      200"));
        assert!(report.contains("hhea: this font's OS/2 table is the old version 0 layout"));
    }

    /// Shifts every table record's offset in a standalone font's bytes by
    /// `delta`, so it can be embedded at a non-zero position inside a `.ttc`
    /// without its table offsets (which are absolute, not relative to the
    /// font's own directory) going stale.
    fn relocate_font(data: &[u8], delta: u32) -> Vec<u8> {
        let mut data = data.to_vec();
        let num_tables = u16::from_be_bytes([data[4], data[5]]) as usize;
        for i in 0..num_tables {
            let base = 12 + i * 16;
            let current = u32::from_be_bytes(data[base + 8..base + 12].try_into().unwrap());
            set_u32(&mut data, base + 8, current + delta);
        }
        data
    }

    /// Builds a two-font `.ttc`: index 0 is `minimal_font` (no OS/2), index
    /// 1 is `font_with_legacy_os2`, so a test can tell which one got read.
    fn collection_with_two_fonts() -> Vec<u8> {
        let header_len = 12 + 2 * 4;
        let font_a = relocate_font(&minimal_font(), header_len as u32);
        let font_b_offset = header_len as u32 + font_a.len() as u32;
        let font_b = relocate_font(&font_with_legacy_os2(), font_b_offset);

        let mut data = vec![0u8; header_len + font_a.len() + font_b.len()];
        data[0..4].copy_from_slice(b"ttcf");
        set_u16(&mut data, 4, 1); // majorVersion
        set_u16(&mut data, 6, 0); // minorVersion
        set_u32(&mut data, 8, 2); // numFonts
        set_u32(&mut data, 12, header_len as u32);
        set_u32(&mut data, 16, font_b_offset);
        data[header_len..header_len + font_a.len()].copy_from_slice(&font_a);
        data[font_b_offset as usize..font_b_offset as usize + font_b.len()].copy_from_slice(&font_b);

        data
    }

    #[test]
    fn reads_metrics_for_a_chosen_font_inside_a_collection() {
        let data = collection_with_two_fonts();

        let report0 = build_report(&data, 0).unwrap();
        assert!(report0.contains("OS/2 table:       not present"));

        let report1 = build_report(&data, 1).unwrap();
        assert!(report1.contains("typo ascender:    800"));
    }

    #[test]
    fn fails_with_a_clear_error_for_a_collection_index_out_of_range() {
        let data = collection_with_two_fonts();
        assert_eq!(
            build_report(&data, 2),
            Err(ParseError::FontIndexOutOfRange { index: 2, count: 2 })
        );
    }
}
