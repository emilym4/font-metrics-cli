use std::env;
use std::fs;
use std::process::ExitCode;

mod sfnt;
mod tables;

use sfnt::ParseError;
use tables::{HeadTable, HheaTable, Os2Table};

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let path = match args.next() {
        Some(p) => p,
        None => {
            eprintln!("usage: fontmetrics <font-file>");
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

    match build_report(&data) {
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
/// bytes instead of files on disk.
fn build_report(data: &[u8]) -> Result<String, ParseError> {
    let directory = sfnt::parse_table_directory(data)?;

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
            out.push_str(&format!("typo ascender:    {}\n", os2.typo_ascender));
            out.push_str(&format!("typo descender:   {}\n", os2.typo_descender));
            out.push_str(&format!("typo line gap:    {}\n", os2.typo_line_gap));
            out.push_str(&format!("win ascent:       {}\n", os2.win_ascent));
            out.push_str(&format!("win descent:      {}\n", os2.win_descent));
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
        let report = build_report(&data).unwrap();

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

        assert_eq!(build_report(&data), Err(ParseError::MissingTable("head")));
    }
}
