//! Parsing for the outer sfnt container shared by TrueType and OpenType
//! fonts: the table directory that maps table tags to byte ranges.
//!
//! Every function here is pure: given the same bytes it always returns the
//! same result, and it never touches the filesystem. That keeps the parser
//! testable with plain byte arrays instead of real font files on disk.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TableRecord {
    pub tag: [u8; 4],
    pub checksum: u32,
    pub offset: u32,
    pub length: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableDirectory {
    pub sfnt_version: u32,
    pub records: Vec<TableRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    TooShort { needed: usize, have: usize },
    MissingTable(&'static str),
    UnexpectedTableLength {
        table: &'static str,
        needed: usize,
        have: usize,
    },
    FontIndexOutOfRange { index: usize, count: usize },
    GlyphIndexOutOfRange { index: u16, count: u16 },
    NoUsableCmapSubtable,
    CharacterNotMapped(char),
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::TooShort { needed, have } => {
                write!(f, "file too short: needed at least {needed} bytes, found {have}")
            }
            ParseError::MissingTable(tag) => {
                write!(f, "font is missing required table '{tag}'")
            }
            ParseError::UnexpectedTableLength { table, needed, have } => {
                write!(f, "table '{table}' too short: needed {needed} bytes, found {have}")
            }
            ParseError::FontIndexOutOfRange { index, count } => {
                write!(f, "font index {index} out of range: file contains {count} font(s)")
            }
            ParseError::GlyphIndexOutOfRange { index, count } => {
                write!(f, "glyph {index} out of range: font contains {count} glyph(s)")
            }
            ParseError::NoUsableCmapSubtable => {
                write!(f, "font's cmap table has no subtable this tool can read (needs format 0, 4, or 12)")
            }
            ParseError::CharacterNotMapped(ch) => {
                write!(f, "character '{ch}' (U+{:04X}) has no glyph in this font's cmap table", *ch as u32)
            }
        }
    }
}

impl std::error::Error for ParseError {}

/// Tag of a TrueType/OpenType font collection header ("ttcf"), read as a
/// big-endian u32 the same way the sfnt version tag is.
const TTC_TAG: u32 = 0x7474_6366;

/// Locates the sfnt table directory for `font_index` within `data`.
///
/// Most font files hold a single font and this is just `font_index == 0`
/// pointing at the start of the file. A `.ttc` collection instead starts
/// with a `ttcf` header followed by one directory offset per font, so this
/// looks at the leading tag to tell the two cases apart.
pub fn parse_font(data: &[u8], font_index: usize) -> Result<TableDirectory, ParseError> {
    require_len(data, 4)?;

    if read_u32(data, 0) == TTC_TAG {
        let offset = ttc_font_offset(data, font_index)?;
        parse_table_directory_at(data, offset as usize)
    } else if font_index == 0 {
        parse_table_directory_at(data, 0)
    } else {
        Err(ParseError::FontIndexOutOfRange { index: font_index, count: 1 })
    }
}

/// Reads offset `font_index` out of a `ttcf` header's OffsetTable.
fn ttc_font_offset(data: &[u8], font_index: usize) -> Result<u32, ParseError> {
    // tag (4) + majorVersion (2) + minorVersion (2) + numFonts (4)
    require_len(data, 12)?;
    let num_fonts = read_u32(data, 8) as usize;

    if font_index >= num_fonts {
        return Err(ParseError::FontIndexOutOfRange { index: font_index, count: num_fonts });
    }

    let entry = 12 + font_index * 4;
    require_len(data, entry + 4)?;
    Ok(read_u32(data, entry))
}

/// Reads the sfnt header and table directory starting at the very front of
/// `data`. Does not validate the contents of any individual table.
pub fn parse_table_directory(data: &[u8]) -> Result<TableDirectory, ParseError> {
    parse_table_directory_at(data, 0)
}

/// Reads an sfnt header and table directory starting at `offset` into
/// `data`, so a single font's directory can be pulled out of a `.ttc`
/// collection instead of assuming it starts at byte 0.
pub fn parse_table_directory_at(data: &[u8], offset: usize) -> Result<TableDirectory, ParseError> {
    let header_end = offset.saturating_add(12);
    require_len(data, header_end)?;

    let sfnt_version = read_u32(data, offset);
    let num_tables = read_u16(data, offset + 4) as usize;

    let header_len = offset + 12 + num_tables * 16;
    require_len(data, header_len)?;

    let mut records = Vec::with_capacity(num_tables);
    for i in 0..num_tables {
        let base = offset + 12 + i * 16;
        let mut tag = [0u8; 4];
        tag.copy_from_slice(&data[base..base + 4]);
        records.push(TableRecord {
            tag,
            checksum: read_u32(data, base + 4),
            offset: read_u32(data, base + 8),
            length: read_u32(data, base + 12),
        });
    }

    Ok(TableDirectory { sfnt_version, records })
}

pub fn find_table(directory: &TableDirectory, tag: &[u8; 4]) -> Option<TableRecord> {
    directory.records.iter().find(|r| &r.tag == tag).copied()
}

/// Identifies which glyph outline format a font uses, based on which
/// outline table its directory lists. This only looks at the directory
/// entries, not the outline data itself - `head`/`hhea`/`OS/2` are laid
/// out identically either way, so the vertical metrics this tool reports
/// don't depend on the answer, but it's useful context in the report.
pub fn outline_format(directory: &TableDirectory) -> &'static str {
    if find_table(directory, b"CFF2").is_some() {
        "CFF2 (PostScript outlines)"
    } else if find_table(directory, b"CFF ").is_some() {
        "CFF (PostScript outlines)"
    } else if find_table(directory, b"glyf").is_some() {
        "TrueType (glyf outlines)"
    } else {
        "unknown"
    }
}

/// Slices out the raw bytes for a table record, bounds-checked against the
/// whole file so a corrupt offset/length can't panic instead of erroring.
pub fn table_bytes<'a>(data: &'a [u8], record: &TableRecord) -> Result<&'a [u8], ParseError> {
    let start = record.offset as usize;
    let end = start.saturating_add(record.length as usize);
    if end > data.len() {
        return Err(ParseError::TooShort { needed: end, have: data.len() });
    }
    Ok(&data[start..end])
}

fn require_len(data: &[u8], needed: usize) -> Result<(), ParseError> {
    if data.len() < needed {
        Err(ParseError::TooShort { needed, have: data.len() })
    } else {
        Ok(())
    }
}

/// Reads a big-endian u16 at `offset`. Callers must have already checked
/// that the slice is long enough; this mirrors how the individual table
/// parsers check their minimum length once up front, then read fixed
/// offsets freely.
pub fn read_u16(data: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes([data[offset], data[offset + 1]])
}

pub fn read_i16(data: &[u8], offset: usize) -> i16 {
    read_u16(data, offset) as i16
}

pub fn read_u32(data: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes([data[offset], data[offset + 1], data[offset + 2], data[offset + 3]])
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

    fn sample_font_with_one_table() -> Vec<u8> {
        // header (12 bytes) + one 16-byte table record + 4 bytes of table data
        let mut data = vec![0u8; 12 + 16 + 4];
        set_u32(&mut data, 0, 0x0001_0000); // sfnt version: TrueType
        set_u16(&mut data, 4, 1); // numTables

        data[12..16].copy_from_slice(b"head");
        set_u32(&mut data, 16, 0); // checksum, unused by the parser
        set_u32(&mut data, 20, 28); // offset: right after the directory
        set_u32(&mut data, 24, 4); // length

        data[28..32].copy_from_slice(&[1, 2, 3, 4]);
        data
    }

    #[test]
    fn parses_a_minimal_table_directory() {
        let data = sample_font_with_one_table();
        let directory = parse_table_directory(&data).unwrap();

        assert_eq!(directory.sfnt_version, 0x0001_0000);
        assert_eq!(directory.records.len(), 1);
        assert_eq!(&directory.records[0].tag, b"head");
    }

    #[test]
    fn finds_a_table_by_tag_and_reads_its_bytes() {
        let data = sample_font_with_one_table();
        let directory = parse_table_directory(&data).unwrap();

        let record = find_table(&directory, b"head").expect("head table present");
        assert_eq!(table_bytes(&data, &record).unwrap(), &[1, 2, 3, 4]);

        assert!(find_table(&directory, b"OS/2").is_none());
    }

    #[test]
    fn rejects_data_shorter_than_the_sfnt_header() {
        let data = [0u8; 8];
        assert_eq!(
            parse_table_directory(&data),
            Err(ParseError::TooShort { needed: 12, have: 8 })
        );
    }

    #[test]
    fn rejects_a_table_record_pointing_past_the_end_of_the_file() {
        let mut data = sample_font_with_one_table();
        set_u32(&mut data, 24, 1000); // claim a length far larger than the file
        let directory = parse_table_directory(&data).unwrap();
        let record = find_table(&directory, b"head").unwrap();

        assert!(table_bytes(&data, &record).is_err());
    }

    /// Builds a `.ttc` with two fonts, each just a bare table directory (no
    /// table data) so the two directories can be told apart by their sfnt
    /// version and number of tables.
    fn sample_collection_with_two_fonts() -> Vec<u8> {
        let header_len = 12 + 2 * 4; // ttcf tag + version + numFonts + 2 offsets
        let font_a_offset = header_len;
        let font_a_len = 12 + 1 * 16;
        let font_b_offset = font_a_offset + font_a_len;
        let font_b_len = 12 + 2 * 16;

        let mut data = vec![0u8; font_b_offset + font_b_len];
        data[0..4].copy_from_slice(b"ttcf");
        set_u16(&mut data, 4, 1); // majorVersion
        set_u16(&mut data, 6, 0); // minorVersion
        set_u32(&mut data, 8, 2); // numFonts
        set_u32(&mut data, 12, font_a_offset as u32);
        set_u32(&mut data, 16, font_b_offset as u32);

        set_u32(&mut data, font_a_offset, 0x0001_0000);
        set_u16(&mut data, font_a_offset + 4, 1);
        data[font_a_offset + 12..font_a_offset + 16].copy_from_slice(b"head");

        set_u32(&mut data, font_b_offset, 0x4f54_544f); // "OTTO"
        set_u16(&mut data, font_b_offset + 4, 2);
        data[font_b_offset + 12..font_b_offset + 16].copy_from_slice(b"CFF ");
        data[font_b_offset + 28..font_b_offset + 32].copy_from_slice(b"head");

        data
    }

    #[test]
    fn parses_each_font_directory_out_of_a_collection() {
        let data = sample_collection_with_two_fonts();

        let font_a = parse_font(&data, 0).unwrap();
        assert_eq!(font_a.sfnt_version, 0x0001_0000);
        assert_eq!(font_a.records.len(), 1);

        let font_b = parse_font(&data, 1).unwrap();
        assert_eq!(font_b.sfnt_version, 0x4f54_544f);
        assert_eq!(font_b.records.len(), 2);
    }

    #[test]
    fn rejects_a_collection_font_index_past_num_fonts() {
        let data = sample_collection_with_two_fonts();
        assert_eq!(
            parse_font(&data, 2),
            Err(ParseError::FontIndexOutOfRange { index: 2, count: 2 })
        );
    }

    #[test]
    fn rejects_a_nonzero_font_index_for_a_plain_non_collection_font() {
        let data = sample_font_with_one_table();
        assert_eq!(
            parse_font(&data, 1),
            Err(ParseError::FontIndexOutOfRange { index: 1, count: 1 })
        );
    }

    #[test]
    fn parses_font_index_zero_for_a_plain_non_collection_font() {
        let data = sample_font_with_one_table();
        let directory = parse_font(&data, 0).unwrap();
        assert_eq!(directory.sfnt_version, 0x0001_0000);
    }

    #[test]
    fn identifies_truetype_outlines_from_a_glyf_table() {
        let data = sample_font_with_one_table();
        let mut directory = parse_font(&data, 0).unwrap();
        directory.records[0].tag = *b"glyf";

        assert_eq!(outline_format(&directory), "TrueType (glyf outlines)");
    }

    #[test]
    fn identifies_cff_outlines_from_a_cff_table() {
        let data = sample_collection_with_two_fonts();
        let font_b = parse_font(&data, 1).unwrap();

        assert_eq!(outline_format(&font_b), "CFF (PostScript outlines)");
    }

    #[test]
    fn prefers_cff2_over_cff_when_both_are_present() {
        let directory = TableDirectory {
            sfnt_version: 0x4f54_544f,
            records: vec![
                TableRecord { tag: *b"CFF ", checksum: 0, offset: 0, length: 0 },
                TableRecord { tag: *b"CFF2", checksum: 0, offset: 0, length: 0 },
            ],
        };

        assert_eq!(outline_format(&directory), "CFF2 (PostScript outlines)");
    }

    #[test]
    fn reports_unknown_outline_format_when_neither_table_is_present() {
        let data = minimal_head_only_directory();
        assert_eq!(outline_format(&data), "unknown");
    }

    fn minimal_head_only_directory() -> TableDirectory {
        TableDirectory {
            sfnt_version: 0x0001_0000,
            records: vec![TableRecord { tag: *b"head", checksum: 0, offset: 0, length: 0 }],
        }
    }
}
