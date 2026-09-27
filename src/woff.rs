//! Parsing for WOFF 1.0 containers: the sfnt table directory plus per-table
//! zlib compression, as defined by the W3C WOFF spec. `to_sfnt` reconstructs
//! a plain sfnt file in memory so the rest of this tool - which only knows
//! the uncompressed table directory format - can read a `.woff` unchanged.
//!
//! WOFF2 uses Brotli instead of zlib, which is a much bigger decoder to
//! write from scratch with no dependencies; that format isn't handled here.

use crate::inflate::{self, InflateError};
use crate::sfnt::{read_u16, read_u32, ParseError};

const WOFF_SIGNATURE: u32 = 0x774F_4646; // "wOFF"
const WOFF2_SIGNATURE: u32 = 0x774F_4632; // "wOF2"

pub fn is_woff(data: &[u8]) -> bool {
    data.len() >= 4 && read_u32(data, 0) == WOFF_SIGNATURE
}

pub fn is_woff2(data: &[u8]) -> bool {
    data.len() >= 4 && read_u32(data, 0) == WOFF2_SIGNATURE
}

struct WoffTableEntry {
    tag: [u8; 4],
    offset: u32,
    comp_length: u32,
    orig_length: u32,
}

const HEADER_LEN: usize = 44;
const ENTRY_LEN: usize = 20;

/// Decompresses a WOFF 1.0 file into a plain sfnt file in memory.
pub fn to_sfnt(data: &[u8]) -> Result<Vec<u8>, ParseError> {
    if data.len() < HEADER_LEN {
        return Err(ParseError::TooShort { needed: HEADER_LEN, have: data.len() });
    }

    let flavor = read_u32(data, 4);
    let num_tables = read_u16(data, 12) as usize;

    let directory_end = HEADER_LEN + num_tables * ENTRY_LEN;
    if data.len() < directory_end {
        return Err(ParseError::TooShort { needed: directory_end, have: data.len() });
    }

    let mut entries = Vec::with_capacity(num_tables);
    for i in 0..num_tables {
        let base = HEADER_LEN + i * ENTRY_LEN;
        let mut tag = [0u8; 4];
        tag.copy_from_slice(&data[base..base + 4]);
        entries.push(WoffTableEntry {
            tag,
            offset: read_u32(data, base + 4),
            comp_length: read_u32(data, base + 8),
            orig_length: read_u32(data, base + 12),
        });
    }

    let mut tables = Vec::with_capacity(num_tables);
    for entry in &entries {
        let start = entry.offset as usize;
        let end = start.saturating_add(entry.comp_length as usize);
        if end > data.len() {
            return Err(ParseError::TooShort { needed: end, have: data.len() });
        }
        let stored = &data[start..end];

        // WOFF only bothers compressing a table when doing so actually
        // saves space; a table stored at its original size is kept raw.
        let table_data = if entry.comp_length == entry.orig_length {
            stored.to_vec()
        } else {
            decompress_zlib(stored, entry.orig_length as usize)?
        };

        if table_data.len() != entry.orig_length as usize {
            return Err(ParseError::Woff(format!(
                "table '{}' decompressed to {} bytes, expected {}",
                String::from_utf8_lossy(&entry.tag),
                table_data.len(),
                entry.orig_length
            )));
        }

        tables.push((entry.tag, table_data));
    }

    Ok(build_sfnt(flavor, &tables))
}

/// Strips the 2-byte zlib header (RFC 1950) and 4-byte trailing Adler-32
/// checksum off `data`, inflates the DEFLATE payload between them, and
/// checks the result against that checksum.
fn decompress_zlib(data: &[u8], expected_len: usize) -> Result<Vec<u8>, ParseError> {
    const ZLIB_HEADER_LEN: usize = 2;
    const ADLER32_LEN: usize = 4;
    if data.len() < ZLIB_HEADER_LEN + ADLER32_LEN {
        return Err(ParseError::Woff("zlib-compressed table shorter than its header and checksum".to_string()));
    }

    let deflate_data = &data[ZLIB_HEADER_LEN..data.len() - ADLER32_LEN];
    let out = inflate::inflate(deflate_data, expected_len).map_err(|e: InflateError| ParseError::Woff(e.to_string()))?;

    let expected_checksum = read_u32(data, data.len() - ADLER32_LEN);
    if adler32(&out) != expected_checksum {
        return Err(ParseError::Woff("zlib checksum mismatch after decompression".to_string()));
    }

    Ok(out)
}

fn adler32(data: &[u8]) -> u32 {
    const MOD_ADLER: u32 = 65521;
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + byte as u32) % MOD_ADLER;
        b = (b + a) % MOD_ADLER;
    }
    (b << 16) | a
}

/// Lays out a plain sfnt file from a flavor tag and a list of already
/// decompressed tables, in the format `sfnt::parse_font` expects: a 12-byte
/// header, one 16-byte record per table, then the table data itself, each
/// table padded up to a 4-byte boundary as the sfnt spec requires.
fn build_sfnt(flavor: u32, tables: &[([u8; 4], Vec<u8>)]) -> Vec<u8> {
    let num_tables = tables.len() as u32;
    let mut max_pow2: u32 = 1;
    let mut entry_selector: u32 = 0;
    while max_pow2 * 2 <= num_tables {
        max_pow2 *= 2;
        entry_selector += 1;
    }
    let search_range = (max_pow2 * 16) as u16;
    let range_shift = (num_tables * 16).saturating_sub(max_pow2 * 16) as u16;

    let header_len = 12 + tables.len() * 16;
    let mut out = vec![0u8; header_len];
    out[0..4].copy_from_slice(&flavor.to_be_bytes());
    out[4..6].copy_from_slice(&(num_tables as u16).to_be_bytes());
    out[6..8].copy_from_slice(&search_range.to_be_bytes());
    out[8..10].copy_from_slice(&entry_selector.to_be_bytes());
    out[10..12].copy_from_slice(&range_shift.to_be_bytes());

    for (i, (tag, data)) in tables.iter().enumerate() {
        let offset = out.len() as u32;
        out.extend_from_slice(data);
        while out.len() % 4 != 0 {
            out.push(0);
        }

        let base = 12 + i * 16;
        out[base..base + 4].copy_from_slice(tag);
        out[base + 4..base + 8].copy_from_slice(&0u32.to_be_bytes()); // checksum: not validated by this tool's own parser
        out[base + 8..base + 12].copy_from_slice(&offset.to_be_bytes());
        out[base + 12..base + 16].copy_from_slice(&(data.len() as u32).to_be_bytes());
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sfnt;

    fn set_u32(buf: &mut [u8], offset: usize, value: u32) {
        buf[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
    }

    fn set_u16(buf: &mut [u8], offset: usize, value: u16) {
        buf[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
    }

    fn write_entry(dir: &mut [u8], index: usize, entry: &WoffTableEntry) {
        let base = HEADER_LEN + index * ENTRY_LEN;
        dir[base..base + 4].copy_from_slice(&entry.tag);
        set_u32(dir, base + 4, entry.offset);
        set_u32(dir, base + 8, entry.comp_length);
        set_u32(dir, base + 12, entry.orig_length);
    }

    /// A zlib stream (RFC 1950 header + a single stored DEFLATE block +
    /// Adler-32 trailer) wrapping `payload`, hand-built the same way a real
    /// WOFF compressor's output would look for data too small to benefit
    /// from actual compression.
    fn zlib_wrap(payload: &[u8]) -> Vec<u8> {
        let mut out = vec![0x78, 0x9C]; // zlib header: deflate, default window/level
        out.push(0x01); // BFINAL=1, BTYPE=00 (stored)
        let len = payload.len() as u16;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(payload);
        out.extend_from_slice(&adler32(payload).to_be_bytes());
        out
    }

    fn minimal_woff(compressed_table: &[u8], raw_table: &[u8]) -> Vec<u8> {
        let num_tables = 2u16;
        let directory_len = HEADER_LEN + num_tables as usize * ENTRY_LEN;
        let compressed_offset = directory_len;
        let raw_offset = compressed_offset + compressed_table.len();
        let total_len = raw_offset + raw_table.len();

        let mut data = vec![0u8; total_len];
        set_u32(&mut data, 0, WOFF_SIGNATURE);
        set_u32(&mut data, 4, 0x0001_0000); // flavor: TrueType
        set_u32(&mut data, 8, total_len as u32);
        set_u16(&mut data, 12, num_tables);

        write_entry(
            &mut data,
            0,
            &WoffTableEntry {
                tag: *b"TEST",
                offset: compressed_offset as u32,
                comp_length: compressed_table.len() as u32,
                orig_length: 2, // "hi", before compression
            },
        );
        write_entry(
            &mut data,
            1,
            &WoffTableEntry {
                tag: *b"RAW1",
                offset: raw_offset as u32,
                comp_length: raw_table.len() as u32,
                orig_length: raw_table.len() as u32,
            },
        );

        data[compressed_offset..compressed_offset + compressed_table.len()].copy_from_slice(compressed_table);
        data[raw_offset..raw_offset + raw_table.len()].copy_from_slice(raw_table);

        data
    }

    #[test]
    fn decompresses_a_zlib_table_and_leaves_a_raw_one_untouched() {
        let compressed = zlib_wrap(b"hi");
        let woff = minimal_woff(&compressed, b"xy");

        let sfnt_data = to_sfnt(&woff).unwrap();
        let directory = sfnt::parse_font(&sfnt_data, 0).unwrap();
        assert_eq!(directory.sfnt_version, 0x0001_0000);

        let compressed_record = sfnt::find_table(&directory, b"TEST").expect("TEST table present");
        assert_eq!(sfnt::table_bytes(&sfnt_data, &compressed_record).unwrap(), b"hi");

        let raw_record = sfnt::find_table(&directory, b"RAW1").expect("RAW1 table present");
        assert_eq!(sfnt::table_bytes(&sfnt_data, &raw_record).unwrap(), b"xy");
    }

    #[test]
    fn recognizes_a_woff_signature() {
        let woff = minimal_woff(&zlib_wrap(b"hi"), b"xy");
        assert!(is_woff(&woff));
        assert!(!is_woff2(&woff));
    }

    #[test]
    fn rejects_a_zlib_table_with_a_corrupt_checksum() {
        let mut compressed = zlib_wrap(b"hi");
        let last = compressed.len() - 1;
        compressed[last] ^= 0xFF; // flip bits in the trailing Adler-32
        let woff = minimal_woff(&compressed, b"xy");

        assert_eq!(
            to_sfnt(&woff),
            Err(ParseError::Woff("zlib checksum mismatch after decompression".to_string()))
        );
    }

    #[test]
    fn rejects_a_file_too_short_for_its_own_table_directory() {
        let data = vec![0u8; HEADER_LEN]; // header claims tables but the file has no directory
        let mut data = data;
        set_u32(&mut data, 0, WOFF_SIGNATURE);
        set_u16(&mut data, 12, 1);

        assert_eq!(to_sfnt(&data), Err(ParseError::TooShort { needed: HEADER_LEN + ENTRY_LEN, have: HEADER_LEN }));
    }
}
