use std::env;
use std::fs;
use std::io::{self, Read};
use std::process::ExitCode;

mod cmap;
mod inflate;
mod sfnt;
mod tables;
mod woff;

use sfnt::ParseError;
use tables::{HeadTable, HheaTable, Os2Table};

fn main() -> ExitCode {
    let mut path = None;
    let mut font_index = 0usize;
    let mut font_size = None;
    let mut glyph_id = None;
    let mut char_arg = None;
    let mut json = false;
    let mut list_tables = false;

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
            "--size" | "-s" => {
                let value = match args.next() {
                    Some(v) => v,
                    None => {
                        eprintln!("fontmetrics: --size needs a number");
                        return ExitCode::FAILURE;
                    }
                };
                font_size = match value.parse::<f64>() {
                    Ok(n) if n > 0.0 && n.is_finite() => Some(n),
                    _ => {
                        eprintln!("fontmetrics: --size must be a positive number, got '{value}'");
                        return ExitCode::FAILURE;
                    }
                };
            }
            "--glyph" | "-g" => {
                let value = match args.next() {
                    Some(v) => v,
                    None => {
                        eprintln!("fontmetrics: --glyph needs a glyph id");
                        return ExitCode::FAILURE;
                    }
                };
                glyph_id = match value.parse() {
                    Ok(n) => Some(n),
                    Err(_) => {
                        eprintln!("fontmetrics: --glyph must be a non-negative integer, got '{value}'");
                        return ExitCode::FAILURE;
                    }
                };
            }
            "--char" | "-c" => {
                let value = match args.next() {
                    Some(v) => v,
                    None => {
                        eprintln!("fontmetrics: --char needs a character");
                        return ExitCode::FAILURE;
                    }
                };
                let mut chars = value.chars();
                char_arg = match (chars.next(), chars.next()) {
                    (Some(c), None) => Some(c),
                    _ => {
                        eprintln!("fontmetrics: --char must be exactly one character, got '{value}'");
                        return ExitCode::FAILURE;
                    }
                };
            }
            "--json" => json = true,
            "--tables" => list_tables = true,
            _ if path.is_none() => path = Some(arg),
            _ => {
                eprintln!("usage: fontmetrics [--index N] [--size SIZE] [--glyph ID | --char CHAR] [--json] [--tables] <font-file>");
                return ExitCode::FAILURE;
            }
        }
    }

    let path = match path {
        Some(p) => p,
        None => {
            eprintln!("usage: fontmetrics [--index N] [--size SIZE] [--glyph ID | --char CHAR] [--json] [--tables] <font-file>");
            return ExitCode::FAILURE;
        }
    };

    if glyph_id.is_some() && char_arg.is_some() {
        eprintln!("fontmetrics: --glyph and --char cannot be combined");
        return ExitCode::FAILURE;
    }

    let data = if path == "-" {
        let mut buf = Vec::new();
        match io::stdin().read_to_end(&mut buf) {
            Ok(_) => buf,
            Err(e) => {
                eprintln!("fontmetrics: cannot read stdin: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        match fs::read(&path) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("fontmetrics: cannot read {path}: {e}");
                return ExitCode::FAILURE;
            }
        }
    };

    let data = if woff::is_woff(&data) {
        match woff::to_sfnt(&data) {
            Ok(sfnt_data) => sfnt_data,
            Err(e) => {
                eprintln!("fontmetrics: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else if woff::is_woff2(&data) {
        eprintln!("fontmetrics: WOFF2 input isn't supported yet (it uses Brotli, not zlib); decompress it to .ttf/.otf first");
        return ExitCode::FAILURE;
    } else {
        data
    };

    let format = if json { OutputFormat::Json } else { OutputFormat::Text };

    let result = if list_tables {
        build_tables_report(&data, font_index, format)
    } else {
        build_report(&data, font_index, font_size, glyph_id, char_arg, format)
    };

    match result {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Text,
    Json,
}

/// Parses the font bytes and renders a metrics report in the requested
/// format. Kept separate from `main` so it can be exercised directly with
/// in-memory bytes instead of files on disk. `font_index` selects which
/// font to read out of a `.ttc` collection; plain single-font files only
/// accept 0. `glyph_id`, if given, also looks up that glyph's advance width
/// from `hmtx`; `char_arg` does the same but resolves the glyph id first via
/// a `cmap` lookup. Callers are expected to pass at most one of the two.
fn build_report(
    data: &[u8],
    font_index: usize,
    font_size: Option<f64>,
    glyph_id: Option<u16>,
    char_arg: Option<char>,
    format: OutputFormat,
) -> Result<String, ParseError> {
    let directory = sfnt::parse_font(data, font_index)?;
    let outline_format = sfnt::outline_format(&directory);

    let head_record = sfnt::find_table(&directory, b"head").ok_or(ParseError::MissingTable("head"))?;
    let head = tables::parse_head(sfnt::table_bytes(data, &head_record)?)?;

    let hhea_record = sfnt::find_table(&directory, b"hhea").ok_or(ParseError::MissingTable("hhea"))?;
    let hhea = tables::parse_hhea(sfnt::table_bytes(data, &hhea_record)?)?;

    let os2 = match sfnt::find_table(&directory, b"OS/2") {
        Some(record) => Some(tables::parse_os2(sfnt::table_bytes(data, &record)?)?),
        None => None,
    };

    let glyph_advance = if let Some(id) = glyph_id {
        Some((id, advance_width_for_glyph(&directory, data, &hhea, id)?, None))
    } else if let Some(ch) = char_arg {
        let cmap_record = sfnt::find_table(&directory, b"cmap").ok_or(ParseError::MissingTable("cmap"))?;
        let id = cmap::find_glyph_id(sfnt::table_bytes(data, &cmap_record)?, ch as u32)?
            .ok_or(ParseError::CharacterNotMapped(ch))?;
        Some((id, advance_width_for_glyph(&directory, data, &hhea, id)?, Some(ch)))
    } else {
        None
    };

    Ok(match format {
        OutputFormat::Text => format_report(&head, &hhea, os2.as_ref(), font_size, glyph_advance, outline_format),
        OutputFormat::Json => format_json(&head, &hhea, os2.as_ref(), font_size, glyph_advance, outline_format),
    })
}

/// Looks up a glyph's advance width from `maxp` and `hmtx`, shared by the
/// `--glyph` (raw id) and `--char` (resolved through `cmap` first) paths.
fn advance_width_for_glyph(directory: &sfnt::TableDirectory, data: &[u8], hhea: &HheaTable, id: u16) -> Result<u16, ParseError> {
    let maxp_record = sfnt::find_table(directory, b"maxp").ok_or(ParseError::MissingTable("maxp"))?;
    let maxp = tables::parse_maxp(sfnt::table_bytes(data, &maxp_record)?)?;
    if id >= maxp.num_glyphs {
        return Err(ParseError::GlyphIndexOutOfRange { index: id, count: maxp.num_glyphs });
    }

    let hmtx_record = sfnt::find_table(directory, b"hmtx").ok_or(ParseError::MissingTable("hmtx"))?;
    let widths = tables::parse_hmtx(sfnt::table_bytes(data, &hmtx_record)?, hhea.number_of_h_metrics, maxp.num_glyphs)?;
    Ok(widths[id as usize])
}

/// Lists every table tag in the font's sfnt directory, in the order they
/// appear on disk. Unlike `build_report`, this doesn't require `head` or
/// `hhea` to be present, so it also works as a quick sanity check on a font
/// that's missing tables the rest of this tool needs.
fn build_tables_report(data: &[u8], font_index: usize, format: OutputFormat) -> Result<String, ParseError> {
    let directory = sfnt::parse_font(data, font_index)?;

    Ok(match format {
        OutputFormat::Text => format_tables_report(&directory),
        OutputFormat::Json => format_tables_json(&directory),
    })
}

fn format_tables_report(directory: &sfnt::TableDirectory) -> String {
    let mut out = format!("tables ({}):\n", directory.records.len());
    for record in &directory.records {
        let tag = String::from_utf8_lossy(&record.tag);
        out.push_str(&format!("  '{tag}'  {} bytes\n", record.length));
    }
    out
}

fn format_tables_json(directory: &sfnt::TableDirectory) -> String {
    let entries: Vec<String> = directory
        .records
        .iter()
        .map(|record| {
            let tag = String::from_utf8_lossy(&record.tag).replace('\\', "\\\\").replace('"', "\\\"");
            format!("{{ \"tag\": \"{tag}\", \"length\": {} }}", record.length)
        })
        .collect();
    format!("{{\n  \"tables\": [\n    {}\n  ]\n}}\n", entries.join(",\n    "))
}

/// Scales a font-units value to the given point size: `value * size /
/// unitsPerEm`, the same formula every renderer uses to go from design units
/// to output pixels.
fn scale(value: i32, units_per_em: u16, font_size: f64) -> f64 {
    value as f64 * font_size / units_per_em as f64
}

fn format_report(
    head: &HeadTable,
    hhea: &HheaTable,
    os2: Option<&Os2Table>,
    font_size: Option<f64>,
    glyph_advance: Option<(u16, u16, Option<char>)>,
    outline_format: &str,
) -> String {
    let mut out = String::new();

    let print_metric = |out: &mut String, label: &str, value: i32| {
        // Every built-in label fits in 18 columns, but a `--char` label
        // carries the character and its codepoint too and can run past
        // that; widen the padding rather than gluing the value onto it.
        let width = label.chars().count().max(17) + 1;
        out.push_str(&format!("{label:<width$}{value}"));
        if let Some(size) = font_size {
            out.push_str(&format!("  ({:.2} at size {})", scale(value, head.units_per_em, size), size));
        }
        out.push('\n');
    };

    out.push_str(&format!("outline format:   {outline_format}\n"));
    out.push_str(&format!("units per em:     {}\n", head.units_per_em));
    print_metric(&mut out, "hhea ascender:", hhea.ascender as i32);
    print_metric(&mut out, "hhea descender:", hhea.descender as i32);
    print_metric(&mut out, "hhea line gap:", hhea.line_gap as i32);

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

            print_metric(&mut out, "typo ascender:", typo_ascender as i32);
            print_metric(&mut out, "typo descender:", typo_descender as i32);
            print_metric(&mut out, "typo line gap:", typo_line_gap as i32);
            print_metric(&mut out, "win ascent:", win_ascent as i32);
            print_metric(&mut out, "win descent:", win_descent as i32);
            if os2.typo_ascender.is_none() {
                out.push_str("  (typo/win fields above are from hhea: this font's OS/2 table is the old version 0 layout without them)\n");
            }
            if let (Some(cap), Some(x)) = (os2.cap_height, os2.x_height) {
                print_metric(&mut out, "cap height:", cap as i32);
                print_metric(&mut out, "x-height:", x as i32);
            }
        }
        None => out.push_str("OS/2 table:       not present\n"),
    }

    if let Some((id, width, ch)) = glyph_advance {
        let label = match ch {
            Some(c) => format!("glyph {id} ('{c}' U+{:04X}) advance:", c as u32),
            None => format!("glyph {id} advance:"),
        };
        print_metric(&mut out, &label, width as i32);
    }

    out
}

/// Renders the same numbers as `format_report`, but as a single JSON object
/// so the output can be piped into another tool instead of parsed as text.
/// Written by hand rather than pulling in a JSON crate: the shape is fixed
/// and every value is a number or bool, so there's no string escaping to
/// get wrong.
fn format_json(
    head: &HeadTable,
    hhea: &HheaTable,
    os2: Option<&Os2Table>,
    font_size: Option<f64>,
    glyph_advance: Option<(u16, u16, Option<char>)>,
    outline_format: &str,
) -> String {
    let metric = |value: i32| -> String {
        match font_size {
            Some(size) => format!("{{ \"value\": {value}, \"scaled\": {} }}", scale(value, head.units_per_em, size)),
            None => value.to_string(),
        }
    };

    let mut fields = vec![
        format!("\"outlineFormat\": \"{outline_format}\""),
        format!("\"unitsPerEm\": {}", head.units_per_em),
        format!("\"hheaAscender\": {}", metric(hhea.ascender as i32)),
        format!("\"hheaDescender\": {}", metric(hhea.descender as i32)),
        format!("\"hheaLineGap\": {}", metric(hhea.line_gap as i32)),
    ];

    let os2_json = match os2 {
        Some(os2) => {
            let typo_ascender = os2.typo_ascender.unwrap_or(hhea.ascender);
            let typo_descender = os2.typo_descender.unwrap_or(hhea.descender);
            let typo_line_gap = os2.typo_line_gap.unwrap_or(hhea.line_gap);
            let win_ascent = os2.win_ascent.unwrap_or_else(|| hhea.ascender.max(0) as u16);
            let win_descent = os2.win_descent.unwrap_or_else(|| hhea.descender.unsigned_abs());

            let mut os2_fields = vec![
                format!("\"typoAscender\": {}", metric(typo_ascender as i32)),
                format!("\"typoDescender\": {}", metric(typo_descender as i32)),
                format!("\"typoLineGap\": {}", metric(typo_line_gap as i32)),
                format!("\"winAscent\": {}", metric(win_ascent as i32)),
                format!("\"winDescent\": {}", metric(win_descent as i32)),
                format!("\"legacyFallback\": {}", os2.typo_ascender.is_none()),
            ];
            if let (Some(cap), Some(x)) = (os2.cap_height, os2.x_height) {
                os2_fields.push(format!("\"capHeight\": {}", metric(cap as i32)));
                os2_fields.push(format!("\"xHeight\": {}", metric(x as i32)));
            }
            format!("{{ {} }}", os2_fields.join(", "))
        }
        None => "null".to_string(),
    };
    fields.push(format!("\"os2\": {os2_json}"));

    if let Some((id, width, ch)) = glyph_advance {
        let mut glyph_fields = vec![format!("\"id\": {id}"), format!("\"advanceWidth\": {}", metric(width as i32))];
        if let Some(c) = ch {
            glyph_fields.push(format!("\"char\": \"{}\"", json_escape_char(c)));
            glyph_fields.push(format!("\"codepoint\": {}", c as u32));
        }
        fields.push(format!("\"glyph\": {{ {} }}", glyph_fields.join(", ")));
    }

    format!("{{\n  {}\n}}\n", fields.join(",\n  "))
}

/// Renders a single character as the body of a JSON string (the part that
/// goes between the quotes). Only escapes what a literal character from
/// `--char` could actually produce: a bare quote, a backslash, or a C0
/// control character - anything else is valid unescaped inside a JSON
/// string.
fn json_escape_char(c: char) -> String {
    match c {
        '"' => "\\\"".to_string(),
        '\\' => "\\\\".to_string(),
        c if (c as u32) < 0x20 => format!("\\u{:04x}", c as u32),
        c => c.to_string(),
    }
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
        let report = build_report(&data, 0, None, None, None, OutputFormat::Text).unwrap();

        assert!(report.contains("units per em:     1000"));
        assert!(report.contains("hhea ascender:    800"));
        assert!(report.contains("hhea descender:   -200"));
        assert!(report.contains("OS/2 table:       not present"));
    }

    #[test]
    fn scales_metrics_to_a_target_font_size() {
        let data = minimal_font();
        let report = build_report(&data, 0, Some(16.0), None, None, OutputFormat::Text).unwrap();

        // 800 units at 1000 unitsPerEm, rendered at size 16: 800 * 16 / 1000 = 12.80
        assert!(report.contains("hhea ascender:    800  (12.80 at size 16)"));
        // -200 units at 1000 unitsPerEm, rendered at size 16: -200 * 16 / 1000 = -3.20
        assert!(report.contains("hhea descender:   -200  (-3.20 at size 16)"));
    }

    #[test]
    fn fails_with_a_clear_error_when_head_is_missing() {
        let mut data = vec![0u8; 12];
        set_u32(&mut data, 0, 0x0001_0000);
        set_u16(&mut data, 4, 0);

        assert_eq!(build_report(&data, 0, None, None, None, OutputFormat::Text), Err(ParseError::MissingTable("head")));
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
        let report = build_report(&data, 0, None, None, None, OutputFormat::Text).unwrap();

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

        let report0 = build_report(&data, 0, None, None, None, OutputFormat::Text).unwrap();
        assert!(report0.contains("OS/2 table:       not present"));

        let report1 = build_report(&data, 1, None, None, None, OutputFormat::Text).unwrap();
        assert!(report1.contains("typo ascender:    800"));
    }

    #[test]
    fn fails_with_a_clear_error_for_a_collection_index_out_of_range() {
        let data = collection_with_two_fonts();
        assert_eq!(
            build_report(&data, 2, None, None, None, OutputFormat::Text),
            Err(ParseError::FontIndexOutOfRange { index: 2, count: 2 })
        );
    }

    #[test]
    fn reports_metrics_as_json_without_os2() {
        let data = minimal_font();
        let report = build_report(&data, 0, None, None, None, OutputFormat::Json).unwrap();

        assert!(report.contains("\"unitsPerEm\": 1000"));
        assert!(report.contains("\"hheaAscender\": 800"));
        assert!(report.contains("\"hheaDescender\": -200"));
        assert!(report.contains("\"os2\": null"));
    }

    #[test]
    fn scales_json_metrics_to_a_target_font_size() {
        let data = minimal_font();
        let report = build_report(&data, 0, Some(16.0), None, None, OutputFormat::Json).unwrap();

        assert!(report.contains("\"hheaAscender\": { \"value\": 800, \"scaled\": 12.8 }"));
        assert!(report.contains("\"hheaDescender\": { \"value\": -200, \"scaled\": -3.2 }"));
    }

    #[test]
    fn reports_os2_fallback_as_json() {
        let data = font_with_legacy_os2();
        let report = build_report(&data, 0, None, None, None, OutputFormat::Json).unwrap();

        assert!(report.contains("\"typoAscender\": 800"));
        assert!(report.contains("\"legacyFallback\": true"));
    }

    /// Builds a font like `minimal_font`, but with a `CFF ` table added to
    /// the directory (with no real CFF data, since only its presence in the
    /// directory matters for outline format detection) so the report can be
    /// checked for an OpenType/CFF flavored font.
    fn font_with_cff_outlines() -> Vec<u8> {
        let head_len = 46;
        let hhea_len = 36;
        let cff_len = 4;
        let head_offset = 12 + 3 * 16;
        let hhea_offset = head_offset + head_len;
        let cff_offset = hhea_offset + hhea_len;

        let mut data = vec![0u8; cff_offset + cff_len];
        set_u32(&mut data, 0, 0x4f54_544f); // "OTTO"
        set_u16(&mut data, 4, 3);

        write_table_record(&mut data, 0, b"head", head_offset as u32, head_len as u32);
        write_table_record(&mut data, 1, b"hhea", hhea_offset as u32, hhea_len as u32);
        write_table_record(&mut data, 2, b"CFF ", cff_offset as u32, cff_len as u32);

        set_u16(&mut data, head_offset + 18, 1000); // unitsPerEm
        set_i16(&mut data, hhea_offset + 4, 800); // ascender
        set_i16(&mut data, hhea_offset + 6, -200); // descender

        data
    }

    #[test]
    fn reports_unknown_outline_format_when_no_outline_table_is_present() {
        let data = minimal_font();
        let report = build_report(&data, 0, None, None, None, OutputFormat::Text).unwrap();

        assert!(report.contains("outline format:   unknown"));
    }

    #[test]
    fn reports_the_outline_format_for_a_cff_flavored_opentype_font() {
        let data = font_with_cff_outlines();
        let report = build_report(&data, 0, None, None, None, OutputFormat::Text).unwrap();

        assert!(report.contains("outline format:   CFF (PostScript outlines)"));
    }

    #[test]
    fn reports_the_outline_format_as_json() {
        let data = font_with_cff_outlines();
        let report = build_report(&data, 0, None, None, None, OutputFormat::Json).unwrap();

        assert!(report.contains("\"outlineFormat\": \"CFF (PostScript outlines)\""));
    }

    /// Builds a font like `minimal_font`, but with `maxp` and `hmtx` tables
    /// added so glyph advance width lookups have something to read. Three
    /// glyphs, two long metrics: glyph 2 reuses glyph 1's advance width.
    fn font_with_hmtx() -> Vec<u8> {
        let head_len = 46;
        let hhea_len = 36;
        let maxp_len = 6;
        let hmtx_len = 2 * 4; // two long metrics, covering all 3 glyphs
        let head_offset = 12 + 4 * 16;
        let hhea_offset = head_offset + head_len;
        let maxp_offset = hhea_offset + hhea_len;
        let hmtx_offset = maxp_offset + maxp_len;

        let mut data = vec![0u8; hmtx_offset + hmtx_len];
        set_u32(&mut data, 0, 0x0001_0000);
        set_u16(&mut data, 4, 4);

        write_table_record(&mut data, 0, b"head", head_offset as u32, head_len as u32);
        write_table_record(&mut data, 1, b"hhea", hhea_offset as u32, hhea_len as u32);
        write_table_record(&mut data, 2, b"maxp", maxp_offset as u32, maxp_len as u32);
        write_table_record(&mut data, 3, b"hmtx", hmtx_offset as u32, hmtx_len as u32);

        set_u16(&mut data, head_offset + 18, 1000); // unitsPerEm
        set_i16(&mut data, hhea_offset + 4, 800); // ascender
        set_i16(&mut data, hhea_offset + 6, -200); // descender
        set_u16(&mut data, hhea_offset + 34, 2); // numberOfHMetrics
        set_u16(&mut data, maxp_offset + 4, 3); // numGlyphs

        set_u16(&mut data, hmtx_offset, 500); // glyph 0 advance width
        set_i16(&mut data, hmtx_offset + 2, 10); // glyph 0 lsb
        set_u16(&mut data, hmtx_offset + 4, 600); // glyph 1 advance width
        set_i16(&mut data, hmtx_offset + 6, 20); // glyph 1 lsb

        data
    }

    #[test]
    fn reports_advance_width_for_a_requested_glyph() {
        let data = font_with_hmtx();
        let report = build_report(&data, 0, None, Some(0), None, OutputFormat::Text).unwrap();

        assert!(report.contains("glyph 0 advance:  500"));
    }

    #[test]
    fn reports_advance_width_for_a_glyph_past_the_long_metrics() {
        let data = font_with_hmtx();
        let report = build_report(&data, 0, None, Some(2), None, OutputFormat::Text).unwrap();

        // Glyph 2 is past numberOfHMetrics (2), so it reuses glyph 1's width.
        assert!(report.contains("glyph 2 advance:  600"));
    }

    #[test]
    fn reports_advance_width_as_json() {
        let data = font_with_hmtx();
        let report = build_report(&data, 0, None, Some(1), None, OutputFormat::Json).unwrap();

        assert!(report.contains("\"glyph\": { \"id\": 1, \"advanceWidth\": 600 }"));
    }

    #[test]
    fn lists_table_tags_present_in_a_font() {
        let data = minimal_font();
        let report = build_tables_report(&data, 0, OutputFormat::Text).unwrap();

        assert!(report.contains("tables (2):"));
        assert!(report.contains("'head'"));
        assert!(report.contains("'hhea'"));
    }

    #[test]
    fn lists_table_tags_without_requiring_head_or_hhea() {
        // build_report would fail on this (no head table), but the table
        // listing only needs the directory itself.
        let mut data = vec![0u8; 12 + 16 + 4];
        set_u32(&mut data, 0, 0x0001_0000);
        set_u16(&mut data, 4, 1);
        write_table_record(&mut data, 0, b"CFF ", 28, 4);

        let report = build_tables_report(&data, 0, OutputFormat::Text).unwrap();
        assert!(report.contains("tables (1):"));
        assert!(report.contains("'CFF '"));
    }

    #[test]
    fn lists_table_tags_as_json() {
        let data = minimal_font();
        let report = build_tables_report(&data, 0, OutputFormat::Json).unwrap();

        assert!(report.contains("\"tag\": \"head\""));
        assert!(report.contains("\"tag\": \"hhea\""));
    }

    #[test]
    fn fails_with_a_clear_error_for_a_glyph_id_out_of_range() {
        let data = font_with_hmtx();
        assert_eq!(
            build_report(&data, 0, None, Some(3), None, OutputFormat::Text),
            Err(ParseError::GlyphIndexOutOfRange { index: 3, count: 3 })
        );
    }

    #[test]
    fn reports_advance_width_for_a_glyph_looked_up_by_character() {
        let data = font_with_cmap();
        let report = build_report(&data, 0, None, None, Some('B'), OutputFormat::Text).unwrap();

        assert!(report.contains("glyph 1 ('B' U+0042) advance: 600"));
    }

    #[test]
    fn reports_advance_width_for_a_character_as_json() {
        let data = font_with_cmap();
        let report = build_report(&data, 0, None, None, Some('B'), OutputFormat::Json).unwrap();

        assert!(report.contains("\"glyph\": { \"id\": 1, \"advanceWidth\": 600, \"char\": \"B\", \"codepoint\": 66 }"));
    }

    #[test]
    fn fails_with_a_clear_error_for_a_character_missing_from_cmap() {
        let data = font_with_cmap();
        assert_eq!(
            build_report(&data, 0, None, None, Some('!'), OutputFormat::Text),
            Err(ParseError::CharacterNotMapped('!'))
        );
    }

    /// Builds a font like `font_with_hmtx`, but with a `cmap` table added
    /// mapping 'A' -> glyph 0 and 'B' -> glyph 1 via a single format 4
    /// subtable, so `--char` lookups have something to resolve against.
    fn font_with_cmap() -> Vec<u8> {
        let head_len = 46;
        let hhea_len = 36;
        let maxp_len = 6;
        let hmtx_len = 2 * 4;
        let seg_count = 2usize; // 'A'-'B', plus the mandatory terminator
        let seg_count_x2 = seg_count * 2;
        let cmap_subtable_len = 16 + seg_count_x2 * 4; // +2 over the cmap.rs helper: includes reservedPad
        let cmap_header_len = 4 + 8; // cmap header + one encoding record
        let cmap_len = cmap_header_len + cmap_subtable_len;

        let head_offset = 12 + 5 * 16;
        let hhea_offset = head_offset + head_len;
        let maxp_offset = hhea_offset + hhea_len;
        let hmtx_offset = maxp_offset + maxp_len;
        let cmap_offset = hmtx_offset + hmtx_len;

        let mut data = vec![0u8; cmap_offset + cmap_len];
        set_u32(&mut data, 0, 0x0001_0000);
        set_u16(&mut data, 4, 5);

        write_table_record(&mut data, 0, b"head", head_offset as u32, head_len as u32);
        write_table_record(&mut data, 1, b"hhea", hhea_offset as u32, hhea_len as u32);
        write_table_record(&mut data, 2, b"maxp", maxp_offset as u32, maxp_len as u32);
        write_table_record(&mut data, 3, b"hmtx", hmtx_offset as u32, hmtx_len as u32);
        write_table_record(&mut data, 4, b"cmap", cmap_offset as u32, cmap_len as u32);

        set_u16(&mut data, head_offset + 18, 1000); // unitsPerEm
        set_i16(&mut data, hhea_offset + 4, 800); // ascender
        set_i16(&mut data, hhea_offset + 6, -200); // descender
        set_u16(&mut data, hhea_offset + 34, 2); // numberOfHMetrics
        set_u16(&mut data, maxp_offset + 4, 2); // numGlyphs

        set_u16(&mut data, hmtx_offset, 500); // glyph 0 ('A') advance width
        set_u16(&mut data, hmtx_offset + 4, 600); // glyph 1 ('B') advance width

        let subtable_offset = cmap_offset + cmap_header_len;
        set_u16(&mut data, cmap_offset + 2, 1); // numTables
        set_u16(&mut data, cmap_offset + 4, 3); // platformID (Windows)
        set_u16(&mut data, cmap_offset + 6, 1); // encodingID (Unicode BMP)
        set_u32(&mut data, cmap_offset + 8, subtable_offset as u32);

        set_u16(&mut data, subtable_offset, 4); // format
        set_u16(&mut data, subtable_offset + 6, seg_count_x2 as u16);

        let end_code_offset = subtable_offset + 14;
        let start_code_offset = end_code_offset + seg_count_x2 + 2;
        let id_delta_offset = start_code_offset + seg_count_x2;
        let id_range_offset_offset = id_delta_offset + seg_count_x2;

        // Segment 0: 'A' (0x41) - 'B' (0x42), glyph id = codepoint - 0x41.
        set_u16(&mut data, end_code_offset, 0x42);
        set_u16(&mut data, start_code_offset, 0x41);
        set_i16(&mut data, id_delta_offset, -0x41);
        set_u16(&mut data, id_range_offset_offset, 0);

        // Segment 1: the mandatory 0xFFFF terminator segment, empty.
        set_u16(&mut data, end_code_offset + 2, 0xFFFF);
        set_u16(&mut data, start_code_offset + 2, 0xFFFF);
        set_i16(&mut data, id_delta_offset + 2, 1);
        set_u16(&mut data, id_range_offset_offset + 2, 0);

        data
    }
}
