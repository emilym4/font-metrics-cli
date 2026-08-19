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
        }
    }
}

impl std::error::Error for ParseError {}

/// Reads the sfnt header and table directory. Does not validate the
/// contents of any individual table.
pub fn parse_table_directory(data: &[u8]) -> Result<TableDirectory, ParseError> {
    require_len(data, 12)?;

    let sfnt_version = read_u32(data, 0);
    let num_tables = read_u16(data, 4) as usize;

    let header_len = 12 + num_tables * 16;
    require_len(data, header_len)?;

    let mut records = Vec::with_capacity(num_tables);
    for i in 0..num_tables {
        let base = 12 + i * 16;
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
}
